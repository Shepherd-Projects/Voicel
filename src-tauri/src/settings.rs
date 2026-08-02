use crate::domain::{
    CURRENT_SCHEMA_VERSION, LoadOutcome, PersistenceError, SETTINGS_FILE_NAME, copy_alias,
    deserialize_value, quarantine_file, read_file, read_json_value, schema_version, storage_path,
    validate_app_data_dir, write_json_atomically,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};

pub const SETTINGS_SCHEMA_VERSION: u32 = CURRENT_SCHEMA_VERSION;
pub const MIN_HISTORY_LIMIT: u8 = 1;
pub const MAX_HISTORY_LIMIT: u8 = 3;

pub type SettingsError = PersistenceError;
pub type SettingsLoadOutcome = LoadOutcome<Settings>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InsertionMethod {
    #[serde(rename = "clipboard_receipt", alias = "clipboard")]
    ClipboardReceipt,
    #[serde(rename = "typing", alias = "literal_typing")]
    Typing,
}

impl Default for InsertionMethod {
    fn default() -> Self {
        Self::ClipboardReceipt
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    #[serde(
        alias = "toggleHotkey",
        alias = "toggleShortcut",
        alias = "toggle_shortcut"
    )]
    pub toggle_hotkey: String,
    #[serde(
        alias = "cancelHotkey",
        alias = "cancelShortcut",
        alias = "cancel_shortcut"
    )]
    pub cancel_hotkey: String,
    #[serde(
        alias = "launchAtStartup",
        alias = "start_at_login",
        alias = "start_on_startup",
        alias = "startup"
    )]
    pub launch_at_startup: bool,
    #[serde(alias = "startHidden", alias = "hidden")]
    pub start_hidden: bool,
    #[serde(alias = "showTrayIcon", alias = "show_in_tray", alias = "tray")]
    pub show_tray_icon: bool,
    #[serde(alias = "unloadImmediately", alias = "immediate_unload")]
    pub unload_immediately: bool,
    #[serde(alias = "insertionMethod")]
    pub insertion_method: InsertionMethod,
    #[serde(alias = "historyLimit")]
    pub history_limit: u8,
    #[serde(alias = "selectedModel")]
    pub selected_model: String,
    #[serde(alias = "customWords")]
    pub custom_words: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            toggle_hotkey: "Ctrl+Shift+Space".to_owned(),
            cancel_hotkey: "Escape".to_owned(),
            launch_at_startup: false,
            start_hidden: true,
            show_tray_icon: true,
            unload_immediately: true,
            insertion_method: InsertionMethod::ClipboardReceipt,
            history_limit: MAX_HISTORY_LIMIT,
            selected_model: String::new(),
            custom_words: Vec::new(),
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), SettingsError> {
        if !(MIN_HISTORY_LIMIT..=MAX_HISTORY_LIMIT).contains(&self.history_limit) {
            return Err(PersistenceError::invalid(
                SETTINGS_FILE_NAME,
                format!(
                    "history_limit must be between {MIN_HISTORY_LIMIT} and {MAX_HISTORY_LIMIT}"
                ),
            ));
        }

        Ok(())
    }

    pub fn normalize_custom_words(&mut self) {
        self.custom_words = normalize_custom_words(&self.custom_words);
    }

    pub fn normalized(mut self) -> Result<Self, SettingsError> {
        self.schema_version = SETTINGS_SCHEMA_VERSION;
        self.normalize_custom_words();
        self.validate()?;
        Ok(self)
    }
}

pub fn normalize_custom_words<I, S>(words: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut normalized = Vec::new();
    let mut keys = Vec::new();

    for word in words {
        let value = word
            .as_ref()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if value.is_empty() {
            continue;
        }

        let key = value.to_lowercase();
        if keys.iter().any(|existing| existing == &key) {
            continue;
        }
        keys.push(key);
        normalized.push(value);
    }

    normalized
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsStore {
    app_data_dir: PathBuf,
}

impl SettingsStore {
    pub fn new(app_data_dir: impl AsRef<Path>) -> Self {
        Self {
            app_data_dir: app_data_dir.as_ref().to_path_buf(),
        }
    }

    pub fn app_data_dir(&self) -> &Path {
        &self.app_data_dir
    }

    pub fn path(&self) -> PathBuf {
        storage_path(&self.app_data_dir, SETTINGS_FILE_NAME)
    }

    pub fn load(&self) -> Result<SettingsLoadOutcome, SettingsError> {
        validate_app_data_dir(&self.app_data_dir)?;
        let path = self.path();
        let Some(bytes) = read_file(&path)? else {
            let settings = Settings::default();
            self.save(&settings)?;
            return Ok(LoadOutcome::Defaulted(settings));
        };

        match decode_settings(&path, &bytes) {
            Ok((settings, migrated)) => {
                if migrated {
                    self.save(&settings)?;
                }
                Ok(LoadOutcome::Loaded(settings))
            }
            Err(error) if error.is_corrupt_document() => {
                let quarantined_path = quarantine_file(&path)?;
                let settings = Settings::default();
                self.save(&settings)?;
                Ok(LoadOutcome::Recovered {
                    value: settings,
                    quarantined_path,
                })
            }
            Err(error) => Err(error),
        }
    }

    pub fn load_or_default(&self) -> Result<SettingsLoadOutcome, SettingsError> {
        self.load()
    }

    pub fn save(&self, settings: &Settings) -> Result<(), SettingsError> {
        validate_app_data_dir(&self.app_data_dir)?;
        let settings = settings.clone().normalized()?;
        write_json_atomically(&self.path(), &settings)
    }
}

pub fn load_settings(app_data_dir: impl AsRef<Path>) -> Result<SettingsLoadOutcome, SettingsError> {
    SettingsStore::new(app_data_dir).load()
}

pub fn save_settings(
    app_data_dir: impl AsRef<Path>,
    settings: &Settings,
) -> Result<(), SettingsError> {
    SettingsStore::new(app_data_dir).save(settings)
}

fn decode_settings(path: &Path, bytes: &[u8]) -> Result<(Settings, bool), SettingsError> {
    let mut value = read_json_value(path, bytes)?;
    let version = schema_version(&value, path)?;
    if version > SETTINGS_SCHEMA_VERSION {
        return Err(PersistenceError::UnsupportedSchema {
            path: path.to_path_buf(),
            found: version,
            supported: SETTINGS_SCHEMA_VERSION,
        });
    }

    let object = value
        .as_object_mut()
        .ok_or_else(|| PersistenceError::invalid(path, "settings JSON must contain an object"))?;
    let migrated = version != SETTINGS_SCHEMA_VERSION
        || !object.contains_key("schema_version")
        || object.contains_key("schemaVersion")
        || has_legacy_setting_keys(object)
        || missing_current_setting_keys(object);

    canonicalize_alias(
        object,
        "toggle_hotkey",
        &["toggleHotkey", "toggleShortcut", "toggle_shortcut"],
    );
    canonicalize_alias(
        object,
        "cancel_hotkey",
        &["cancelHotkey", "cancelShortcut", "cancel_shortcut"],
    );
    canonicalize_alias(
        object,
        "launch_at_startup",
        &[
            "launchAtStartup",
            "start_at_login",
            "start_on_startup",
            "startup",
        ],
    );
    canonicalize_alias(object, "start_hidden", &["startHidden", "hidden"]);
    canonicalize_alias(
        object,
        "show_tray_icon",
        &["showTrayIcon", "show_in_tray", "tray"],
    );
    canonicalize_alias(
        object,
        "unload_immediately",
        &["unloadImmediately", "immediate_unload"],
    );
    canonicalize_alias(object, "insertion_method", &["insertionMethod"]);
    canonicalize_alias(object, "history_limit", &["historyLimit"]);
    canonicalize_alias(object, "selected_model", &["selectedModel"]);
    canonicalize_alias(object, "custom_words", &["customWords"]);
    canonicalize_alias(object, "schema_version", &["schemaVersion"]);
    object.insert(
        "schema_version".to_owned(),
        Value::from(SETTINGS_SCHEMA_VERSION),
    );

    let mut settings: Settings = deserialize_value(path, value)?;
    let original_words = settings.custom_words.clone();
    settings.schema_version = SETTINGS_SCHEMA_VERSION;
    settings.normalize_custom_words();
    settings.validate()?;
    let words_changed = original_words != settings.custom_words;

    Ok((settings, migrated || words_changed))
}

fn has_legacy_setting_keys(object: &serde_json::Map<String, Value>) -> bool {
    [
        "toggleHotkey",
        "toggleShortcut",
        "toggle_shortcut",
        "cancelHotkey",
        "cancelShortcut",
        "cancel_shortcut",
        "launchAtStartup",
        "start_at_login",
        "start_on_startup",
        "startup",
        "startHidden",
        "hidden",
        "showTrayIcon",
        "show_in_tray",
        "tray",
        "unloadImmediately",
        "immediate_unload",
        "insertionMethod",
        "historyLimit",
        "selectedModel",
        "customWords",
    ]
    .iter()
    .any(|key| object.contains_key(*key))
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

fn missing_current_setting_keys(object: &serde_json::Map<String, Value>) -> bool {
    [
        "toggle_hotkey",
        "cancel_hotkey",
        "launch_at_startup",
        "start_hidden",
        "show_tray_icon",
        "unload_immediately",
        "insertion_method",
        "history_limit",
        "selected_model",
        "custom_words",
    ]
    .iter()
    .any(|key| !object.contains_key(*key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_app_data() -> PathBuf {
        let token = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "voicel-settings-test-{}-{token}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test app-data directory");
        path
    }

    fn remove_test_directory(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn roundtrip_preserves_settings() {
        let app_data = temporary_app_data();
        let store = SettingsStore::new(&app_data);
        let settings = Settings {
            toggle_hotkey: "Ctrl+Alt+R".to_owned(),
            cancel_hotkey: "Ctrl+Alt+C".to_owned(),
            launch_at_startup: true,
            start_hidden: true,
            show_tray_icon: false,
            unload_immediately: false,
            insertion_method: InsertionMethod::Typing,
            history_limit: 2,
            selected_model: "small.en".to_owned(),
            custom_words: vec!["Voicel".to_owned(), "Rust".to_owned()],
            ..Settings::default()
        };

        store.save(&settings).expect("save settings");
        let outcome = store.load().expect("load settings");
        assert_eq!(outcome, LoadOutcome::Loaded(settings));

        remove_test_directory(&app_data);
    }

    #[test]
    fn missing_fields_default_and_legacy_fields_migrate_losslessly() {
        let app_data = temporary_app_data();
        let store = SettingsStore::new(&app_data);
        fs::write(
            store.path(),
            serde_json::to_vec_pretty(&json!({
                "toggleShortcut": "F9",
                "launchAtStartup": true,
                "historyLimit": 2,
                "selectedModel": "tiny.en",
                "customWords": ["  Open   AI ", "open ai", "", "Rust"]
            }))
            .expect("serialize legacy settings"),
        )
        .expect("write legacy settings");

        let settings = store.load().expect("migrate settings").into_value();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.toggle_hotkey, "F9");
        assert!(settings.launch_at_startup);
        assert_eq!(settings.history_limit, 2);
        assert_eq!(settings.selected_model, "tiny.en");
        assert_eq!(settings.cancel_hotkey, Settings::default().cancel_hotkey);
        assert_eq!(settings.custom_words, vec!["Open AI", "Rust"]);

        let persisted: Value =
            serde_json::from_slice(&fs::read(store.path()).expect("read migrated settings"))
                .expect("parse migrated settings");
        assert_eq!(persisted["schema_version"], SETTINGS_SCHEMA_VERSION);
        assert_eq!(persisted["toggle_hotkey"], "F9");

        remove_test_directory(&app_data);
    }

    #[test]
    fn corrupt_json_is_quarantined_and_returns_recovery_outcome() {
        let app_data = temporary_app_data();
        let store = SettingsStore::new(&app_data);
        fs::write(store.path(), b"{ not valid json").expect("write corrupt settings");

        let outcome = store.load().expect("recover corrupt settings");
        let quarantined_path = outcome
            .quarantined_path()
            .expect("recovery outcome contains quarantine path")
            .to_path_buf();
        assert!(outcome.was_recovered());
        assert!(quarantined_path.exists());
        if store.path().exists() {
            let recovered: Settings =
                serde_json::from_slice(&fs::read(store.path()).expect("read recovered settings"))
                    .expect("parse recovered settings");
            assert_eq!(recovered, Settings::default());
        }
        assert_eq!(outcome.value(), &Settings::default());

        remove_test_directory(&app_data);
    }

    #[test]
    fn replacement_is_atomic_from_the_store_perspective() {
        let app_data = temporary_app_data();
        let store = SettingsStore::new(&app_data);
        let first = Settings::default();
        let second = Settings {
            selected_model: "medium.en".to_owned(),
            ..first.clone()
        };

        store.save(&first).expect("save first settings");
        store.save(&second).expect("replace settings");
        assert_eq!(
            store.load().expect("load replaced settings").value(),
            &second
        );

        let temporary_files = fs::read_dir(&app_data)
            .expect("read app-data directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(temporary_files, 0);

        remove_test_directory(&app_data);
    }

    #[test]
    fn custom_words_are_trimmed_collapsed_and_case_insensitively_deduplicated() {
        let words = vec![
            "  New   Word ".to_owned(),
            "new word".to_owned(),
            " ".to_owned(),
            "Voicel".to_owned(),
            "VOICEL".to_owned(),
        ];
        assert_eq!(normalize_custom_words(&words), vec!["New Word", "Voicel"]);
    }
}
