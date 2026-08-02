mod app_state;
mod audio;
mod domain;
mod engine;
mod history;
mod insertion;
mod models;
mod modifier_shortcut;
mod session;
mod settings;

use app_state::{
    AppState, add_custom_word, app_snapshot, clear_history, copy_text, install_model,
    remove_custom_word, select_model, set_shortcut_capture, update_settings,
};
use session::{
    SessionManager, cancel_recording, cancel_session, start_recording, start_session,
    stop_recording, stop_session,
};
use tauri::{Manager, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use modifier_shortcut::{
    ModifierBindings, ModifierShortcutManager, ParsedShortcut, parse_shortcut, shortcuts_are_equal,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            let model_dir = app.path().app_local_data_dir()?.join("models");
            let state = AppState::load(app_data_dir, model_dir)
                .map_err(|error| std::io::Error::other(format!("initialize Voicel: {error}")))?;
            let start_hidden = state.settings.lock().start_hidden;
            app.manage(state);
            app.manage(SessionManager::new());
            let modifier_shortcuts = ModifierShortcutManager::start(app.handle().clone())
                .map_err(std::io::Error::other)?;
            app.manage(modifier_shortcuts);
            let settings = app.state::<AppState>().settings.lock().clone();
            register_global_shortcuts(
                app.handle(),
                &settings.toggle_hotkey,
                &settings.cancel_hotkey,
            )
            .map_err(std::io::Error::other)?;
            setup_tray(app)?;
            set_tray_visible(app.handle(), settings.show_tray_icon)
                .map_err(std::io::Error::other)?;
            if start_hidden {
                if let Some(window) = app.get_webview_window("main") {
                    window.hide()?;
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main"
                && let WindowEvent::CloseRequested { api, .. } = event
            {
                let keep_in_tray = window
                    .app_handle()
                    .state::<AppState>()
                    .settings
                    .lock()
                    .show_tray_icon;
                if keep_in_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_snapshot,
            update_settings,
            add_custom_word,
            remove_custom_word,
            select_model,
            install_model,
            copy_text,
            clear_history,
            set_shortcut_capture,
            start_recording,
            stop_recording,
            cancel_recording,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Voicel");
}

pub(crate) fn validate_global_shortcuts(toggle: &str, cancel: &str) -> Result<(), String> {
    let toggle = parse_shortcut(toggle, "recording")?;
    let cancel = parse_shortcut(cancel, "cancel")?;
    if shortcuts_are_equal(toggle, cancel) {
        return Err("Recording and cancel shortcuts must be different".to_owned());
    }
    Ok(())
}

pub(crate) fn register_global_shortcuts(
    app: &tauri::AppHandle,
    toggle: &str,
    cancel: &str,
) -> Result<(), String> {
    let toggle = parse_shortcut(toggle, "recording")?;
    let cancel = parse_shortcut(cancel, "cancel")?;
    if shortcuts_are_equal(toggle, cancel) {
        return Err("Recording and cancel shortcuts must be different".to_owned());
    }

    let shortcuts = app.global_shortcut();
    shortcuts
        .unregister_all()
        .map_err(|error| error.to_string())?;

    if let ParsedShortcut::Standard(toggle) = toggle
        && let Err(error) = shortcuts.on_shortcut(toggle, |app, _, event| {
            if event.state == ShortcutState::Pressed {
                dispatch_toggle(app.clone());
            }
        })
    {
        return Err(format!("Register recording shortcut: {error}"));
    }
    if let ParsedShortcut::Standard(cancel) = cancel
        && let Err(error) = shortcuts.on_shortcut(cancel, |app, _, event| {
            if event.state == ShortcutState::Pressed {
                dispatch_cancel(app.clone());
            }
        })
    {
        let _ = shortcuts.unregister_all();
        return Err(format!("Register cancel shortcut: {error}"));
    }

    app.state::<ModifierShortcutManager>()
        .configure(ModifierBindings {
            toggle: toggle.modifier_chord(),
            cancel: cancel.modifier_chord(),
        });
    Ok(())
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn validates_regular_and_modifier_only_shortcuts_together() {
        assert!(validate_global_shortcuts("Ctrl+Alt", "Escape").is_ok());
        assert!(validate_global_shortcuts("Ctrl+Shift+Space", "Alt+Meta").is_ok());
    }

    #[test]
    fn rejects_matching_modifier_only_shortcuts() {
        let error = validate_global_shortcuts("Ctrl+Alt", "alt+ctrl")
            .expect_err("matching shortcuts must be rejected");
        assert_eq!(error, "Recording and cancel shortcuts must be different");
    }

    #[test]
    fn preserves_default_shortcuts() {
        assert!(validate_global_shortcuts("Ctrl+Shift+Space", "Escape").is_ok());
    }
}

pub(crate) fn set_tray_visible(app: &tauri::AppHandle, visible: bool) -> Result<(), String> {
    app.tray_by_id("voicel-tray")
        .ok_or_else(|| "Voicel tray icon is unavailable".to_owned())?
        .set_visible(visible)
        .map_err(|error| error.to_string())
}

pub(crate) fn dispatch_toggle(app: tauri::AppHandle) {
    let scheduler = app.clone();
    if let Err(error) = scheduler.run_on_main_thread(move || {
        let state = app.state::<AppState>();
        if state
            .shortcut_capture_active
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let sessions = app.state::<SessionManager>();
        let phase = *state.phase.lock();
        let result = if matches!(
            phase,
            app_state::AppPhase::Loading
                | app_state::AppPhase::Recording
                | app_state::AppPhase::Finalizing
        ) {
            stop_session(&sessions)
        } else {
            start_session(app.clone(), &state, &sessions)
        };
        if let Err(error) = result {
            log::error!("global recording shortcut: {error}");
        }
    }) {
        log::error!("schedule global recording shortcut: {error}");
    }
}

pub(crate) fn dispatch_cancel(app: tauri::AppHandle) {
    let scheduler = app.clone();
    if let Err(error) = scheduler.run_on_main_thread(move || {
        let state = app.state::<AppState>();
        if state
            .shortcut_capture_active
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let sessions = app.state::<SessionManager>();
        if let Err(error) = cancel_session(&sessions) {
            log::error!("global cancel shortcut: {error}");
        }
    }) {
        log::error!("schedule global cancel shortcut: {error}");
    }
}

fn setup_tray(app: &mut tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;

    let open = MenuItem::with_id(app, "open", "Open Voicel", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::with_id("voicel-tray")
        .menu(&menu)
        .tooltip("Voicel — ready to dictate")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
