use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::ffi::OsStr;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;
pub const SETTINGS_FILE_NAME: &str = "settings.json";
pub const HISTORY_FILE_NAME: &str = "history.json";
pub const MAX_HISTORY_ENTRIES: usize = 3;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TranscriptRecord {
    pub text: String,
    #[serde(alias = "created_at", alias = "createdAt")]
    pub timestamp: String,
    #[serde(alias = "model_name", alias = "modelName")]
    pub model: String,
}

pub type HistoryEntry = TranscriptRecord;
pub type Transcript = TranscriptRecord;

impl TranscriptRecord {
    pub fn new(
        text: impl Into<String>,
        timestamp: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            text: text.into(),
            timestamp: timestamp.into(),
            model: model.into(),
        }
    }

    pub fn copy_target(&self) -> &str {
        &self.text
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSession {
    pub text: String,
    pub timestamp: String,
    pub model: String,
    pub canceled: bool,
    pub insertion_failed: bool,
}

impl TranscriptSession {
    pub fn completed(
        text: impl Into<String>,
        timestamp: impl Into<String>,
        model: impl Into<String>,
        insertion_failed: bool,
    ) -> Self {
        Self {
            text: text.into(),
            timestamp: timestamp.into(),
            model: model.into(),
            canceled: false,
            insertion_failed,
        }
    }

    pub fn canceled(
        text: impl Into<String>,
        timestamp: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            text: text.into(),
            timestamp: timestamp.into(),
            model: model.into(),
            canceled: true,
            insertion_failed: false,
        }
    }
}

#[derive(Debug)]
pub enum PersistenceError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    InvalidDocument {
        path: PathBuf,
        message: String,
    },
    UnsupportedSchema {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    Quarantine {
        source_path: PathBuf,
        destination: PathBuf,
        source: io::Error,
    },
}

impl PersistenceError {
    pub(crate) fn invalid(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::InvalidDocument {
            path: path.into(),
            message: message.into(),
        }
    }

    pub(crate) fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }

    pub(crate) fn is_corrupt_document(&self) -> bool {
        matches!(self, Self::Json { .. } | Self::InvalidDocument { .. })
    }
}

impl std::fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "failed to {operation} {}: {source}",
                path.display()
            ),
            Self::Json { path, source } => {
                write!(formatter, "invalid JSON in {}: {source}", path.display())
            }
            Self::InvalidDocument { path, message } => {
                write!(formatter, "invalid document {}: {message}", path.display())
            }
            Self::UnsupportedSchema {
                path,
                found,
                supported,
            } => write!(
                formatter,
                "unsupported schema version {found} in {} (supported through {supported})",
                path.display()
            ),
            Self::Quarantine {
                source_path,
                destination,
                source,
            } => write!(
                formatter,
                "failed to quarantine {} as {}: {source}",
                source_path.display(),
                destination.display()
            ),
        }
    }
}

impl std::error::Error for PersistenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::Quarantine { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::InvalidDocument { .. } | Self::UnsupportedSchema { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome<T> {
    Loaded(T),
    Defaulted(T),
    Recovered { value: T, quarantined_path: PathBuf },
}

impl<T> LoadOutcome<T> {
    pub fn value(&self) -> &T {
        match self {
            Self::Loaded(value) | Self::Defaulted(value) => value,
            Self::Recovered { value, .. } => value,
        }
    }

    pub fn into_value(self) -> T {
        match self {
            Self::Loaded(value) | Self::Defaulted(value) => value,
            Self::Recovered { value, .. } => value,
        }
    }

    pub fn was_recovered(&self) -> bool {
        matches!(self, Self::Recovered { .. })
    }

    pub fn quarantined_path(&self) -> Option<&Path> {
        match self {
            Self::Recovered {
                quarantined_path, ..
            } => Some(quarantined_path),
            Self::Loaded(_) | Self::Defaulted(_) => None,
        }
    }
}

pub(crate) fn storage_path(app_data_dir: impl AsRef<Path>, file_name: &str) -> PathBuf {
    app_data_dir.as_ref().join(file_name)
}

pub(crate) fn validate_app_data_dir(path: &Path) -> Result<(), PersistenceError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(PersistenceError::invalid(
            path,
            "application data path must be absolute",
        ))
    }
}

pub(crate) fn read_json_value(path: &Path, bytes: &[u8]) -> Result<Value, PersistenceError> {
    serde_json::from_slice(bytes).map_err(|source| PersistenceError::Json {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn deserialize_value<T: DeserializeOwned>(
    path: &Path,
    value: Value,
) -> Result<T, PersistenceError> {
    serde_json::from_value(value).map_err(|source| PersistenceError::Json {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn schema_version(value: &Value, path: &Path) -> Result<u32, PersistenceError> {
    let Some(object) = value.as_object() else {
        return Err(PersistenceError::invalid(
            path,
            "top-level JSON value must be an object",
        ));
    };

    let version = object
        .get("schema_version")
        .or_else(|| object.get("schemaVersion"));

    match version {
        None => Ok(0),
        Some(value) => value
            .as_u64()
            .and_then(|number| u32::try_from(number).ok())
            .ok_or_else(|| {
                PersistenceError::invalid(path, "schema_version must be an unsigned 32-bit integer")
            }),
    }
}

pub(crate) fn copy_alias(
    object: &mut serde_json::Map<String, Value>,
    canonical: &str,
    aliases: &[&str],
) {
    if !object.contains_key(canonical) {
        for alias in aliases {
            if let Some(value) = object.get(*alias).cloned() {
                object.insert(canonical.to_owned(), value);
                break;
            }
        }
    }

    for alias in aliases {
        object.remove(*alias);
    }
}

pub(crate) fn read_file(path: &Path) -> Result<Option<Vec<u8>>, PersistenceError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(PersistenceError::io("read", path, source)),
    }
}

pub(crate) fn write_json_atomically<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<(), PersistenceError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| PersistenceError::Json {
        path: path.to_path_buf(),
        source,
    })?;

    let parent = path.parent().ok_or_else(|| {
        PersistenceError::invalid(path, "persistent file path must have a parent directory")
    })?;
    fs::create_dir_all(parent)
        .map_err(|source| PersistenceError::io("create app-data directory", parent, source))?;

    let mut allocated_temporary_path = None;
    let mut temporary_file = None;
    for attempt in 0..32u32 {
        let candidate = temporary_path(path, attempt);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                allocated_temporary_path = Some(candidate);
                temporary_file = Some(file);
                break;
            }
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(PersistenceError::io(
                    "create temporary file",
                    candidate,
                    source,
                ));
            }
        }
    }

    let temporary_path = allocated_temporary_path.ok_or_else(|| {
        PersistenceError::invalid(path, "could not allocate a unique temporary file")
    })?;
    let mut temporary_file = temporary_file.expect("temporary file exists when its path exists");

    let write_result = temporary_file
        .write_all(&bytes)
        .and_then(|()| temporary_file.sync_all());
    drop(temporary_file);
    if let Err(source) = write_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(PersistenceError::io(
            "write temporary file",
            temporary_path,
            source,
        ));
    }

    if let Err(source) = atomic_replace(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(PersistenceError::io(
            "replace persistent file",
            path,
            source,
        ));
    }

    sync_parent_directory(parent);
    Ok(())
}

pub(crate) fn quarantine_file(path: &Path) -> Result<PathBuf, PersistenceError> {
    let parent = path.parent().ok_or_else(|| {
        PersistenceError::invalid(path, "corrupt file path must have a parent directory")
    })?;
    let name = path.file_name().unwrap_or_else(|| OsStr::new("document"));
    let stamp = timestamp_token();

    for attempt in 0..32u32 {
        let destination = parent.join(format!(
            "{}.corrupt-{}-{}",
            name.to_string_lossy(),
            stamp,
            attempt
        ));
        match fs::rename(path, &destination) {
            Ok(()) => {
                sync_parent_directory(parent);
                return Ok(destination);
            }
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(PersistenceError::Quarantine {
                    source_path: path.to_path_buf(),
                    destination,
                    source,
                });
            }
        }
    }

    Err(PersistenceError::invalid(
        path,
        "could not allocate a unique quarantine path",
    ))
}

fn temporary_path(path: &Path, attempt: u32) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().unwrap_or_else(|| OsStr::new("document"));
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".{}.tmp-{}-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        timestamp_token(),
        counter.wrapping_add(u64::from(attempt))
    ))
}

fn timestamp_token() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) {
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) {}

#[cfg(not(windows))]
fn atomic_replace(temporary_path: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(temporary_path, destination)
}

#[cfg(windows)]
fn atomic_replace(temporary_path: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let temporary = temporary_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    let succeeded = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_path_uses_only_the_supplied_app_data_directory() {
        let app_data = Path::new("C:/app-data");
        assert_eq!(
            storage_path(app_data, SETTINGS_FILE_NAME),
            PathBuf::from("C:/app-data/settings.json")
        );
        assert!(!storage_path(app_data, SETTINGS_FILE_NAME).starts_with("C:/Program Files"));
    }
}
