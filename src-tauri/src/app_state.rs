use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::history::HistoryStore;
use crate::models::{ModelInfo, ModelStore};
use crate::settings::{InsertionMethod, Settings, SettingsStore};

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AppPhase {
    Loading,
    #[default]
    Ready,
    Recording,
    Finalizing,
    Error,
}

#[derive(Clone, Debug, Default)]
pub struct TranscriptState {
    pub stable_text: String,
    pub revising_text: String,
    pub elapsed_ms: u64,
    pub input_level: f32,
    pub error: Option<String>,
}

pub struct AppState {
    pub settings: Mutex<Settings>,
    pub settings_store: SettingsStore,
    pub history_store: HistoryStore,
    pub model_store: ModelStore,
    pub phase: Mutex<AppPhase>,
    pub transcript: Mutex<TranscriptState>,
    pub shortcut_capture_active: AtomicBool,
}

impl AppState {
    pub fn load(app_data_dir: PathBuf, model_dir: PathBuf) -> Result<Self, String> {
        let settings_store = SettingsStore::new(&app_data_dir);
        let mut settings = settings_store
            .load()
            .map_err(|error| error.to_string())?
            .into_value();
        if settings.selected_model.is_empty() {
            settings.selected_model = "zipformer-en".to_owned();
            settings_store
                .save(&settings)
                .map_err(|error| error.to_string())?;
        }
        Ok(Self {
            history_store: HistoryStore::new(&app_data_dir),
            model_store: ModelStore::new(model_dir),
            settings: Mutex::new(settings),
            settings_store,
            phase: Mutex::new(AppPhase::Ready),
            transcript: Mutex::new(TranscriptState::default()),
            shortcut_capture_active: AtomicBool::new(false),
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    phase: AppPhase,
    active_model: String,
    models: Vec<ModelInfo>,
    history: Vec<HistoryItem>,
    custom_words: Vec<String>,
    stable_text: String,
    revising_text: String,
    elapsed_ms: u64,
    input_level: f32,
    settings: SettingsView,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryItem {
    id: String,
    text: String,
    created_at: String,
    model_name: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    toggle_shortcut: String,
    cancel_shortcut: String,
    launch_at_startup: bool,
    start_hidden: bool,
    show_tray_icon: bool,
    unload_immediately: bool,
    insertion_method: InsertionMethod,
    history_limit: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    toggle_shortcut: Option<String>,
    cancel_shortcut: Option<String>,
    launch_at_startup: Option<bool>,
    start_hidden: Option<bool>,
    show_tray_icon: Option<bool>,
    unload_immediately: Option<bool>,
    insertion_method: Option<InsertionMethod>,
    history_limit: Option<u8>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelDownloadProgress {
    model_id: String,
    progress: u8,
}

#[tauri::command]
pub fn app_snapshot(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    let settings = state.settings.lock().clone();
    let history = state
        .history_store
        .load(settings.history_limit)
        .map_err(|error| error.to_string())?
        .into_value();
    let transcript = state.transcript.lock().clone();
    let history = history
        .entries
        .into_iter()
        .enumerate()
        .map(|(index, item)| HistoryItem {
            id: format!("{}-{index}", item.timestamp),
            text: item.text,
            created_at: item.timestamp,
            model_name: item.model,
        })
        .collect();

    Ok(AppSnapshot {
        phase: *state.phase.lock(),
        active_model: settings.selected_model.clone(),
        models: state.model_store.catalog(),
        history,
        custom_words: settings.custom_words.clone(),
        stable_text: transcript.stable_text,
        revising_text: transcript.revising_text,
        elapsed_ms: transcript.elapsed_ms,
        input_level: transcript.input_level,
        error: transcript.error,
        settings: settings_view(&settings),
    })
}

#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    patch: SettingsPatch,
) -> Result<SettingsView, String> {
    let previous = state.settings.lock().clone();
    let mut settings = previous.clone();
    if let Some(value) = patch.toggle_shortcut {
        settings.toggle_hotkey = value;
    }
    if let Some(value) = patch.cancel_shortcut {
        settings.cancel_hotkey = value;
    }
    if let Some(value) = patch.launch_at_startup {
        settings.launch_at_startup = value;
    }
    if let Some(value) = patch.start_hidden {
        settings.start_hidden = value;
    }
    if let Some(value) = patch.show_tray_icon {
        settings.show_tray_icon = value;
    }
    if let Some(value) = patch.unload_immediately {
        settings.unload_immediately = value;
    }
    if let Some(value) = patch.insertion_method {
        settings.insertion_method = value;
    }
    if let Some(value) = patch.history_limit {
        settings.history_limit = value;
    }
    settings = settings.normalized().map_err(|error| error.to_string())?;
    crate::validate_global_shortcuts(&settings.toggle_hotkey, &settings.cancel_hotkey)?;
    set_launch_at_startup(&app, settings.launch_at_startup)?;
    if let Err(error) = crate::set_tray_visible(&app, settings.show_tray_icon) {
        let _ = set_launch_at_startup(&app, previous.launch_at_startup);
        return Err(error);
    }
    if let Err(error) =
        crate::register_global_shortcuts(&app, &settings.toggle_hotkey, &settings.cancel_hotkey)
    {
        let _ = set_launch_at_startup(&app, previous.launch_at_startup);
        let _ = crate::set_tray_visible(&app, previous.show_tray_icon);
        let _ = crate::register_global_shortcuts(
            &app,
            &previous.toggle_hotkey,
            &previous.cancel_hotkey,
        );
        return Err(error);
    }
    if let Err(error) = state.settings_store.save(&settings) {
        let _ = set_launch_at_startup(&app, previous.launch_at_startup);
        let _ = crate::set_tray_visible(&app, previous.show_tray_icon);
        let _ = crate::register_global_shortcuts(
            &app,
            &previous.toggle_hotkey,
            &previous.cancel_hotkey,
        );
        return Err(error.to_string());
    }
    *state.settings.lock() = settings.clone();
    Ok(settings_view(&settings))
}

fn set_launch_at_startup(app: &AppHandle, enabled: bool) -> Result<(), String> {
    if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    }
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn add_custom_word(state: State<'_, AppState>, word: String) -> Result<(), String> {
    let mut settings = state.settings.lock().clone();
    settings.custom_words.push(word);
    settings = settings.normalized().map_err(|error| error.to_string())?;
    state
        .settings_store
        .save(&settings)
        .map_err(|error| error.to_string())?;
    *state.settings.lock() = settings;
    Ok(())
}

#[tauri::command]
pub fn remove_custom_word(state: State<'_, AppState>, word: String) -> Result<(), String> {
    let mut settings = state.settings.lock().clone();
    settings
        .custom_words
        .retain(|candidate| !candidate.eq_ignore_ascii_case(&word));
    state
        .settings_store
        .save(&settings)
        .map_err(|error| error.to_string())?;
    *state.settings.lock() = settings;
    Ok(())
}

#[tauri::command]
pub fn select_model(state: State<'_, AppState>, model_id: String) -> Result<(), String> {
    let spec = state
        .model_store
        .spec(&model_id)
        .map_err(|error| error.to_string())?;
    if !state.model_store.is_installed(spec) {
        return Err(format!("Install {} before selecting it", spec.name));
    }
    let mut settings = state.settings.lock().clone();
    settings.selected_model = model_id;
    state
        .settings_store
        .save(&settings)
        .map_err(|error| error.to_string())?;
    *state.settings.lock() = settings;
    Ok(())
}

#[tauri::command]
pub async fn install_model(
    app: AppHandle,
    state: State<'_, AppState>,
    model_id: String,
) -> Result<(), String> {
    state
        .model_store
        .install(&model_id, |progress| {
            let _ = app.emit(
                "model-download-progress",
                ModelDownloadProgress {
                    model_id: model_id.clone(),
                    progress,
                },
            );
        })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn copy_text(app: AppHandle, text: String) -> Result<(), String> {
    app.clipboard()
        .write_text(text)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    let history_limit = state.settings.lock().history_limit;
    state
        .history_store
        .clear(history_limit)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_shortcut_capture(state: State<'_, AppState>, active: bool) {
    state
        .shortcut_capture_active
        .store(active, Ordering::Release);
}

fn settings_view(settings: &Settings) -> SettingsView {
    SettingsView {
        toggle_shortcut: settings.toggle_hotkey.clone(),
        cancel_shortcut: settings.cancel_hotkey.clone(),
        launch_at_startup: settings.launch_at_startup,
        start_hidden: settings.start_hidden,
        show_tray_icon: settings.show_tray_icon,
        unload_immediately: settings.unload_immediately,
        insertion_method: settings.insertion_method,
        history_limit: settings.history_limit,
    }
}
