use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use bzip2::read::BzDecoder;
use futures_util::StreamExt;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineKind {
    OnlineZipformer,
    OfflineParakeet,
}

#[derive(Clone, Copy, Debug)]
pub struct ModelSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub streaming: StreamingClass,
    pub languages: &'static str,
    pub size_label: &'static str,
    pub archive_url: &'static str,
    pub directory: &'static str,
    pub expected_files: &'static [&'static str],
    pub engine: EngineKind,
    pub license: &'static str,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamingClass {
    True,
    Incremental,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub streaming: StreamingClass,
    pub languages: String,
    pub size_label: String,
    pub installed: bool,
}

const ZIPFORMER_FILES: &[&str] = &[
    "encoder-epoch-99-avg-1.int8.onnx",
    "decoder-epoch-99-avg-1.onnx",
    "joiner-epoch-99-avg-1.int8.onnx",
    "tokens.txt",
];

const PARAKEET_FILES: &[&str] = &[
    "encoder.int8.onnx",
    "decoder.int8.onnx",
    "joiner.int8.onnx",
    "tokens.txt",
];

pub const MODEL_CATALOG: &[ModelSpec] = &[
    ModelSpec {
        id: "zipformer-en",
        name: "Live English",
        description: "Low-latency CPU transcription that decodes continuously as audio arrives.",
        streaming: StreamingClass::True,
        languages: "English",
        size_label: "About 510 MB",
        archive_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-2023-06-21.tar.bz2",
        directory: "sherpa-onnx-streaming-zipformer-en-2023-06-21",
        expected_files: ZIPFORMER_FILES,
        engine: EngineKind::OnlineZipformer,
        license: "Apache-2.0 model packaging; source training data licenses apply",
    },
    ModelSpec {
        id: "parakeet-v3",
        name: "Parakeet v3",
        description: "Higher-accuracy multilingual transcription with a continuously revised live preview.",
        streaming: StreamingClass::Incremental,
        languages: "25 European languages",
        size_label: "About 640 MB",
        archive_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
        directory: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
        expected_files: PARAKEET_FILES,
        engine: EngineKind::OfflineParakeet,
        license: "CC-BY-4.0",
    },
];

#[derive(Clone, Debug)]
pub struct ModelStore {
    root: PathBuf,
}

impl ModelStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn catalog(&self) -> Vec<ModelInfo> {
        MODEL_CATALOG
            .iter()
            .map(|spec| ModelInfo {
                id: spec.id.to_owned(),
                name: spec.name.to_owned(),
                description: spec.description.to_owned(),
                streaming: spec.streaming,
                languages: spec.languages.to_owned(),
                size_label: spec.size_label.to_owned(),
                installed: self.is_installed(spec),
            })
            .collect()
    }

    pub fn spec(&self, id: &str) -> Result<&'static ModelSpec> {
        MODEL_CATALOG
            .iter()
            .find(|spec| spec.id == id)
            .ok_or_else(|| anyhow::anyhow!("Unknown model: {id}"))
    }

    pub fn model_path(&self, spec: &ModelSpec) -> PathBuf {
        self.root.join(spec.directory)
    }

    pub fn is_installed(&self, spec: &ModelSpec) -> bool {
        let directory = self.model_path(spec);
        spec.expected_files
            .iter()
            .all(|name| valid_model_file(&directory.join(name)))
    }

    pub async fn install<F>(&self, id: &str, mut progress: F) -> Result<()>
    where
        F: FnMut(u8) + Send,
    {
        let spec = self.spec(id)?;
        if self.is_installed(spec) {
            progress(100);
            return Ok(());
        }

        fs::create_dir_all(&self.root).context("create model directory")?;
        let archive = self.root.join(format!(".{}.download", spec.id));
        if !valid_model_file(&archive) {
            let response = reqwest::get(spec.archive_url)
                .await
                .with_context(|| format!("download {}", spec.name))?
                .error_for_status()
                .with_context(|| format!("model server rejected {}", spec.name))?;
            let total = response.content_length();
            let mut file = File::create(&archive).context("create model download")?;
            let mut received = 0_u64;
            let mut stream = response.bytes_stream();

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.context("read model download")?;
                received += chunk.len() as u64;
                file.write_all(&chunk).context("write model download")?;
                if let Some(total) = total.filter(|value| *value > 0) {
                    progress(((received.saturating_mul(90) / total).min(90)) as u8);
                }
            }
            file.sync_all().context("flush model download")?;
            drop(file);
        }
        progress(90);

        let extraction_root = self.root.join(format!(".{}.installing", spec.id));
        remove_directory_if_present(&extraction_root)?;
        fs::create_dir_all(&extraction_root).context("create model staging directory")?;
        if let Err(error) = extract_bzip_tar(&archive, &extraction_root) {
            let _ = fs::remove_file(&archive);
            return Err(error.context("model archive was incomplete; retry the download"));
        }

        let extracted = extraction_root.join(spec.directory);
        verify_expected_files(&extracted, spec.expected_files)?;
        let destination = self.model_path(spec);
        remove_directory_if_present(&destination)?;
        fs::rename(&extracted, &destination).context("activate downloaded model")?;
        remove_directory_if_present(&extraction_root)?;
        fs::remove_file(&archive).context("remove model archive")?;
        progress(100);
        Ok(())
    }
}

fn extract_bzip_tar(archive_path: &Path, destination: &Path) -> Result<()> {
    let archive_file = File::open(archive_path).context("open model archive")?;
    let decoder = BzDecoder::new(archive_file);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries().context("read model archive")? {
        let mut entry = entry.context("read model archive entry")?;
        if !entry.unpack_in(destination).context("extract model file")? {
            bail!("Model archive contains an unsafe path");
        }
    }
    Ok(())
}

fn verify_expected_files(directory: &Path, expected: &[&str]) -> Result<()> {
    for name in expected {
        let path = directory.join(name);
        if !valid_model_file(&path) {
            bail!("Downloaded model is missing {}", path.display());
        }
    }
    Ok(())
}

fn valid_model_file(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

fn remove_directory_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_distinguishes_real_and_incremental_streaming() {
        assert!(matches!(MODEL_CATALOG[0].streaming, StreamingClass::True));
        assert!(matches!(
            MODEL_CATALOG[1].streaming,
            StreamingClass::Incremental
        ));
        assert_eq!(MODEL_CATALOG[1].engine, EngineKind::OfflineParakeet);
    }

    #[test]
    fn unknown_model_is_rejected() {
        let store = ModelStore::new(PathBuf::from("models"));
        assert!(store.spec("imaginary").is_err());
    }
}
