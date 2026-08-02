use crate::domain::{
    CURRENT_SCHEMA_VERSION, HISTORY_FILE_NAME, HistoryEntry, LoadOutcome, PersistenceError,
    TranscriptRecord, TranscriptSession, copy_alias, deserialize_value, quarantine_file, read_file,
    read_json_value, schema_version, storage_path, validate_app_data_dir, write_json_atomically,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};

pub const HISTORY_SCHEMA_VERSION: u32 = CURRENT_SCHEMA_VERSION;

pub type HistoryError = PersistenceError;
pub type HistoryLoadOutcome = LoadOutcome<History>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub schema_version: u32,
    #[serde(alias = "items", alias = "history", alias = "transcripts")]
    pub entries: Vec<HistoryEntry>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            schema_version: HISTORY_SCHEMA_VERSION,
            entries: Vec::new(),
        }
    }
}

impl History {
    pub fn new(entries: Vec<HistoryEntry>) -> Self {
        Self {
            schema_version: HISTORY_SCHEMA_VERSION,
            entries,
        }
    }

    pub fn trim_to_limit(&mut self, history_limit: u8) -> Result<bool, HistoryError> {
        validate_history_limit(history_limit)?;
        self.schema_version = HISTORY_SCHEMA_VERSION;

        let original_len = self.entries.len();
        self.entries.retain(|entry| !entry.text.trim().is_empty());
        self.entries.truncate(usize::from(history_limit));
        Ok(original_len != self.entries.len())
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn copy_target(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(TranscriptRecord::copy_target)
    }

    pub fn get_copy_target(&self, index: usize) -> Option<&str> {
        self.copy_target(index)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryStore {
    app_data_dir: PathBuf,
}

impl HistoryStore {
    pub fn new(app_data_dir: impl AsRef<Path>) -> Self {
        Self {
            app_data_dir: app_data_dir.as_ref().to_path_buf(),
        }
    }

    pub fn app_data_dir(&self) -> &Path {
        &self.app_data_dir
    }

    pub fn path(&self) -> PathBuf {
        storage_path(&self.app_data_dir, HISTORY_FILE_NAME)
    }

    pub fn load(&self, history_limit: u8) -> Result<HistoryLoadOutcome, HistoryError> {
        validate_app_data_dir(&self.app_data_dir)?;
        validate_history_limit(history_limit)?;
        let path = self.path();
        let Some(bytes) = read_file(&path)? else {
            let history = History::default();
            self.save(&history, history_limit)?;
            return Ok(LoadOutcome::Defaulted(history));
        };

        match decode_history(&path, &bytes, history_limit) {
            Ok((history, migrated)) => {
                if migrated {
                    self.save(&history, history_limit)?;
                }
                Ok(LoadOutcome::Loaded(history))
            }
            Err(error) if error.is_corrupt_document() => {
                let quarantined_path = quarantine_file(&path)?;
                let history = History::default();
                self.save(&history, history_limit)?;
                Ok(LoadOutcome::Recovered {
                    value: history,
                    quarantined_path,
                })
            }
            Err(error) => Err(error),
        }
    }

    pub fn save(&self, history: &History, history_limit: u8) -> Result<(), HistoryError> {
        validate_app_data_dir(&self.app_data_dir)?;
        validate_history_limit(history_limit)?;
        let mut history = history.clone();
        history.trim_to_limit(history_limit)?;
        write_json_atomically(&self.path(), &history)
    }

    pub fn append(
        &self,
        entry: TranscriptRecord,
        history_limit: u8,
    ) -> Result<Option<TranscriptRecord>, HistoryError> {
        validate_history_limit(history_limit)?;
        if entry.text.trim().is_empty() {
            return Ok(None);
        }

        let mut history = self.load(history_limit)?.into_value();
        history.entries.insert(0, entry.clone());
        history.trim_to_limit(history_limit)?;
        self.save(&history, history_limit)?;
        Ok(Some(entry))
    }

    pub fn append_completed(
        &self,
        text: impl Into<String>,
        timestamp: impl Into<String>,
        model: impl Into<String>,
        history_limit: u8,
    ) -> Result<Option<TranscriptRecord>, HistoryError> {
        self.record_completed(text, timestamp, model, history_limit)
    }

    pub fn record_completed(
        &self,
        text: impl Into<String>,
        timestamp: impl Into<String>,
        model: impl Into<String>,
        history_limit: u8,
    ) -> Result<Option<TranscriptRecord>, HistoryError> {
        self.append(
            TranscriptRecord {
                text: text.into(),
                timestamp: timestamp.into(),
                model: model.into(),
            },
            history_limit,
        )
    }

    pub fn record_session(
        &self,
        session: &TranscriptSession,
        history_limit: u8,
    ) -> Result<Option<TranscriptRecord>, HistoryError> {
        validate_history_limit(history_limit)?;
        if session.canceled || session.text.trim().is_empty() {
            return Ok(None);
        }

        self.record_completed(
            session.text.clone(),
            session.timestamp.clone(),
            session.model.clone(),
            history_limit,
        )
    }

    pub fn clear(&self, history_limit: u8) -> Result<(), HistoryError> {
        self.save(&History::default(), history_limit)
    }

    pub fn copy_target(
        &self,
        index: usize,
        history_limit: u8,
    ) -> Result<Option<String>, HistoryError> {
        Ok(self
            .load(history_limit)?
            .value()
            .copy_target(index)
            .map(str::to_owned))
    }
}

pub fn load_history(
    app_data_dir: impl AsRef<Path>,
    history_limit: u8,
) -> Result<HistoryLoadOutcome, HistoryError> {
    HistoryStore::new(app_data_dir).load(history_limit)
}

pub fn validate_history_limit(history_limit: u8) -> Result<(), HistoryError> {
    if !(1..=3).contains(&history_limit) {
        return Err(PersistenceError::invalid(
            HISTORY_FILE_NAME,
            "history_limit must be between 1 and 3",
        ));
    }
    Ok(())
}

fn decode_history(
    path: &Path,
    bytes: &[u8],
    history_limit: u8,
) -> Result<(History, bool), HistoryError> {
    let mut value = read_json_value(path, bytes)?;
    let mut migrated = false;
    let version = if value.is_array() {
        migrated = true;
        let entries = std::mem::take(&mut value);
        value = json!({
            "schema_version": HISTORY_SCHEMA_VERSION,
            "entries": entries,
        });
        0
    } else {
        schema_version(&value, path)?
    };

    if version > HISTORY_SCHEMA_VERSION {
        return Err(PersistenceError::UnsupportedSchema {
            path: path.to_path_buf(),
            found: version,
            supported: HISTORY_SCHEMA_VERSION,
        });
    }

    let object = value.as_object_mut().ok_or_else(|| {
        PersistenceError::invalid(path, "history JSON must contain an object or array")
    })?;
    migrated |= version != HISTORY_SCHEMA_VERSION
        || !object.contains_key("schema_version")
        || object.contains_key("schemaVersion")
        || object.contains_key("items")
        || object.contains_key("history")
        || object.contains_key("transcripts")
        || !object.contains_key("entries");
    canonicalize_alias(object, "entries", &["items", "history", "transcripts"]);
    canonicalize_alias(object, "schema_version", &["schemaVersion"]);
    object.insert(
        "schema_version".to_owned(),
        Value::from(HISTORY_SCHEMA_VERSION),
    );

    let mut history: History = deserialize_value(path, value)?;
    history.schema_version = HISTORY_SCHEMA_VERSION;
    migrated |= history.trim_to_limit(history_limit)?;
    Ok((history, migrated))
}

fn canonicalize_alias(
    object: &mut serde_json::Map<String, Value>,
    canonical: &str,
    aliases: &[&str],
) {
    copy_alias(object, canonical, aliases);
    for alias in aliases {
        object.remove(*alias);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_app_data() -> PathBuf {
        let token = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "voicel-history-test-{}-{token}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test app-data directory");
        path
    }

    fn remove_test_directory(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn history_is_newest_first_and_bounded_to_three_entries() {
        let app_data = temporary_app_data();
        let store = HistoryStore::new(&app_data);

        for index in 1..=4 {
            store
                .record_completed(
                    format!("transcript {index}"),
                    format!("2026-08-02T00:00:0{index}Z"),
                    "small.en",
                    3,
                )
                .expect("record transcript");
        }

        let history = store.load(3).expect("load bounded history").into_value();
        assert_eq!(history.entries.len(), 3);
        assert_eq!(
            history
                .entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            vec!["transcript 4", "transcript 3", "transcript 2"]
        );
        assert_eq!(history.entries[0].timestamp, "2026-08-02T00:00:04Z");
        assert_eq!(history.entries[0].model, "small.en");

        remove_test_directory(&app_data);
    }

    #[test]
    fn completed_transcripts_survive_insertion_failure_but_cancelled_and_empty_do_not() {
        let app_data = temporary_app_data();
        let store = HistoryStore::new(&app_data);

        let insertion_failed = TranscriptSession::completed("kept", "now", "base", true);
        let cancelled = TranscriptSession::canceled("discarded", "later", "base");
        let empty = TranscriptSession::completed("  ", "later", "base", false);

        assert_eq!(
            store
                .record_session(&insertion_failed, 3)
                .expect("record failed insertion")
                .unwrap()
                .text,
            "kept"
        );
        assert!(
            store
                .record_session(&cancelled, 3)
                .expect("skip cancelled")
                .is_none()
        );
        assert!(
            store
                .record_session(&empty, 3)
                .expect("skip empty")
                .is_none()
        );

        let history = store.load(3).expect("load history").into_value();
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].text, "kept");

        remove_test_directory(&app_data);
    }

    #[test]
    fn legacy_array_history_migrates_and_defaults_schema() {
        let app_data = temporary_app_data();
        let store = HistoryStore::new(&app_data);
        fs::write(
            store.path(),
            serde_json::to_vec_pretty(&vec![serde_json::json!({
                "text": "legacy",
                "createdAt": "2026-08-02T00:00:00Z",
                "modelName": "tiny.en"
            })])
            .expect("serialize legacy history"),
        )
        .expect("write legacy history");

        let history = store.load(3).expect("migrate history").into_value();
        assert_eq!(history.schema_version, HISTORY_SCHEMA_VERSION);
        assert_eq!(history.entries[0].timestamp, "2026-08-02T00:00:00Z");
        assert_eq!(history.entries[0].model, "tiny.en");

        remove_test_directory(&app_data);
    }

    #[test]
    fn copy_target_returns_only_the_requested_transcript_text() {
        let app_data = temporary_app_data();
        let store = HistoryStore::new(&app_data);
        store
            .record_completed("copy me", "now", "small.en", 3)
            .expect("record transcript");

        assert_eq!(
            store.copy_target(0, 3).expect("load copy target"),
            Some("copy me".to_owned())
        );
        assert_eq!(store.copy_target(1, 3).expect("load missing target"), None);

        remove_test_directory(&app_data);
    }
}
