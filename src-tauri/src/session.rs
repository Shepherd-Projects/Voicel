use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::app_state::{AppPhase, AppState, TranscriptState};
use crate::audio::{AudioCapture, AudioChunk};
use crate::domain::TranscriptSession;
use crate::engine::{SpeechEngine, TranscriptRevision};
use crate::insertion::{paste_preserving_clipboard, type_text};
use crate::models::ModelSpec;
use crate::settings::{InsertionMethod, Settings};

pub const TRANSCRIPT_REVISION_EVENT: &str = "transcript-revision";
pub const SESSION_PHASE_EVENT: &str = "session-phase";
pub const OVERLAY_WINDOW_LABEL: &str = "overlay";

const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(40);
const CONTROL_BUFFER_SIZE: usize = 1;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptRevisionEvent {
    pub revision: u64,
    pub stable_text: String,
    pub revising_text: String,
    #[serde(rename = "final")]
    pub is_final: bool,
    pub elapsed_ms: u64,
    pub input_level: f32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPhaseEvent {
    pub phase: AppPhase,
}

#[derive(Default)]
pub struct SessionManager {
    inner: parking_lot::Mutex<ManagerState>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    fn reserve(&self) -> Result<SessionReservation, String> {
        let (control_sender, control_receiver) = mpsc::sync_channel(CONTROL_BUFFER_SIZE);
        let mut manager = self.inner.lock();
        if manager.active.is_some() {
            return Err("A recording session is already active".to_owned());
        }

        let id = manager.next_id;
        manager.next_id = manager.next_id.wrapping_add(1);
        manager.active = Some(ActiveSession { id, control_sender });
        Ok(SessionReservation {
            id,
            control_receiver,
        })
    }

    fn send_control(&self, command: SessionCommand) -> Result<(), String> {
        let control_sender = self
            .inner
            .lock()
            .active
            .as_ref()
            .map(|active| active.control_sender.clone());
        let Some(control_sender) = control_sender else {
            return Ok(());
        };

        match control_sender.try_send(command) {
            Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => Ok(()),
        }
    }

    fn clear(&self, id: u64) {
        let mut manager = self.inner.lock();
        if manager
            .active
            .as_ref()
            .is_some_and(|active| active.id == id)
        {
            manager.active = None;
        }
    }
}

struct ManagerState {
    next_id: u64,
    active: Option<ActiveSession>,
}

impl Default for ManagerState {
    fn default() -> Self {
        Self {
            next_id: 1,
            active: None,
        }
    }
}

struct ActiveSession {
    id: u64,
    control_sender: SyncSender<SessionCommand>,
}

struct SessionReservation {
    id: u64,
    control_receiver: Receiver<SessionCommand>,
}

struct ActiveCancelShortcut {
    app: AppHandle,
}

impl ActiveCancelShortcut {
    fn register(app: &AppHandle) -> Result<Self, String> {
        crate::set_cancel_shortcut_active(app, true)?;
        Ok(Self { app: app.clone() })
    }
}

impl Drop for ActiveCancelShortcut {
    fn drop(&mut self) {
        if let Err(error) = crate::set_cancel_shortcut_active(&self.app, false) {
            log::error!("release global cancel shortcut: {error}");
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionCommand {
    Stop,
    Cancel,
}

#[derive(Clone)]
struct SessionSettings {
    model_name: String,
    insertion_method: InsertionMethod,
    history_limit: u8,
}

struct PreparedSession {
    engine: SpeechEngine,
    capture: AudioCapture,
    audio_receiver: Receiver<AudioChunk>,
    settings: SessionSettings,
}

#[derive(Clone, Debug)]
struct TranscriptProgress {
    revision: u64,
    stable_text: String,
    revising_text: String,
    elapsed_ms: u64,
    input_level: f32,
}

impl Default for TranscriptProgress {
    fn default() -> Self {
        Self {
            revision: 0,
            stable_text: String::new(),
            revising_text: String::new(),
            elapsed_ms: 0,
            input_level: 0.0,
        }
    }
}

enum RecordingResult {
    Stop(TranscriptProgress),
    Cancel,
    Failure {
        progress: TranscriptProgress,
        error: String,
    },
}

struct StopCompletion {
    progress: TranscriptProgress,
    errors: Vec<String>,
    finalization_failed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(windows)]
struct ForegroundWindow(isize);

#[cfg(not(windows))]
type ForegroundWindow = isize;

#[tauri::command]
pub fn start_recording(
    app: AppHandle,
    state: State<'_, AppState>,
    sessions: State<'_, SessionManager>,
) -> Result<(), String> {
    start_session(app, &state, &sessions)
}

#[tauri::command]
pub fn stop_recording(sessions: State<'_, SessionManager>) -> Result<(), String> {
    stop_session(&sessions)
}

#[tauri::command]
pub fn cancel_recording(sessions: State<'_, SessionManager>) -> Result<(), String> {
    cancel_session(&sessions)
}

pub fn start_session(
    app: AppHandle,
    state: &AppState,
    sessions: &SessionManager,
) -> Result<(), String> {
    let reservation = sessions.reserve()?;
    reset_for_start(state);
    let target_window = match current_foreground_window() {
        Ok(target_window) => target_window,
        Err(error) => return fail_to_start(state, sessions, reservation.id, error),
    };
    let cancel_shortcut = match ActiveCancelShortcut::register(&app) {
        Ok(cancel_shortcut) => cancel_shortcut,
        Err(error) => return fail_to_start(state, sessions, reservation.id, error),
    };
    if let Err(error) = show_overlay(&app) {
        return fail_to_start(state, sessions, reservation.id, error);
    }
    if let Err(error) = publish_phase(&app, state, AppPhase::Loading) {
        let _ = hide_overlay(&app);
        let _ = restore_target_focus(target_window);
        return fail_to_start(state, sessions, reservation.id, error);
    }

    let session_id = reservation.id;
    let control_receiver = reservation.control_receiver;
    let worker_app = app.clone();
    let spawn_result = thread::Builder::new()
        .name("voicel-session".to_owned())
        .spawn(move || {
            let _cancel_shortcut = cancel_shortcut;
            run_session_worker(worker_app.clone(), target_window, control_receiver);
            worker_app.state::<SessionManager>().clear(session_id);
        });

    if let Err(error) = spawn_result {
        let _ = hide_overlay(&app);
        let _ = restore_target_focus(target_window);
        return fail_to_start(
            state,
            sessions,
            session_id,
            format!("Start recording worker: {error}"),
        );
    }

    Ok(())
}

pub fn stop_session(sessions: &SessionManager) -> Result<(), String> {
    sessions.send_control(SessionCommand::Stop)
}

pub fn cancel_session(sessions: &SessionManager) -> Result<(), String> {
    sessions.send_control(SessionCommand::Cancel)
}

fn run_session_worker(
    app: AppHandle,
    target_window: ForegroundWindow,
    control_receiver: Receiver<SessionCommand>,
) {
    let state = app.state::<AppState>();
    let preparation =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prepare_session(&state)));
    let prepared = match preparation {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(error)) => {
            fail_worker_start(&app, &state, target_window, error);
            return;
        }
        Err(_) => {
            fail_worker_start(
                &app,
                &state,
                target_window,
                "Speech model initialization stopped unexpectedly".to_owned(),
            );
            return;
        }
    };

    match control_receiver.try_recv() {
        Ok(SessionCommand::Stop | SessionCommand::Cancel) => {
            let PreparedSession {
                engine, capture, ..
            } = prepared;
            drop(capture);
            engine.unload();
            finish_cancel(&app, &state, target_window);
            return;
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
            drop(prepared);
            fail_worker_start(
                &app,
                &state,
                target_window,
                "Recording control channel closed while loading".to_owned(),
            );
            return;
        }
    }

    if let Err(error) = publish_phase(&app, &state, AppPhase::Recording) {
        drop(prepared);
        fail_worker_start(&app, &state, target_window, error);
        return;
    }
    if let Err(error) = publish_progress(&app, &state, &TranscriptProgress::default(), false) {
        drop(prepared);
        fail_worker_start(&app, &state, target_window, error);
        return;
    }

    let PreparedSession {
        engine,
        capture,
        audio_receiver,
        settings,
        ..
    } = prepared;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_worker(
            app.clone(),
            target_window,
            engine,
            capture,
            audio_receiver,
            control_receiver,
            settings,
        );
    }));
    if result.is_err() {
        let _ = hide_overlay(&app);
        let _ = restore_target_focus(target_window);
        expose_error(&state, "Recording session stopped unexpectedly".to_owned());
    }
}

fn prepare_session(state: &AppState) -> Result<PreparedSession, String> {
    let settings = state.settings.lock().clone();
    let spec = state
        .model_store
        .spec(&settings.selected_model)
        .map_err(|error| format!("Load selected model: {error}"))?;
    if !state.model_store.is_installed(spec) {
        return Err(format!("Install {} before recording", spec.name));
    }

    let model_directory = state.model_store.model_path(spec);
    let engine = SpeechEngine::load(spec, model_directory, &settings.custom_words)
        .map_err(|error| format!("Load model '{}': {error}", spec.name))?;
    let (capture, audio_receiver) =
        AudioCapture::start().map_err(|error| format!("Start microphone capture: {error}"))?;

    Ok(PreparedSession {
        engine,
        capture,
        audio_receiver,
        settings: session_settings(spec, &settings),
    })
}

fn fail_worker_start(
    app: &AppHandle,
    state: &AppState,
    target_window: ForegroundWindow,
    error: String,
) {
    let mut errors = vec![error];
    if let Err(error) = hide_overlay(app) {
        errors.push(error);
    }
    if let Err(error) = restore_target_focus(target_window) {
        errors.push(error);
    }
    expose_error(state, errors.join("; "));
}

fn session_settings(spec: &ModelSpec, settings: &Settings) -> SessionSettings {
    SessionSettings {
        model_name: spec.name.to_owned(),
        insertion_method: settings.insertion_method,
        history_limit: settings.history_limit,
    }
}

fn run_worker(
    app: AppHandle,
    target_window: ForegroundWindow,
    mut engine: SpeechEngine,
    capture: AudioCapture,
    audio_receiver: Receiver<AudioChunk>,
    control_receiver: Receiver<SessionCommand>,
    settings: SessionSettings,
) {
    let state = app.state::<AppState>();
    let recording =
        record_until_control(&app, &state, &mut engine, audio_receiver, control_receiver);
    drop(capture);

    match recording {
        RecordingResult::Cancel => {
            engine.unload();
            finish_cancel(&app, &state, target_window);
        }
        RecordingResult::Stop(progress) => {
            let completion = finish_stop(&app, &state, &mut engine, progress);
            engine.unload();
            finish_stop_after_unload(&app, &state, target_window, settings, completion);
        }
        RecordingResult::Failure { progress, error } => {
            engine.unload();
            finish_failure(
                &app,
                &state,
                target_window,
                settings,
                StopCompletion {
                    progress,
                    errors: vec![error],
                    finalization_failed: true,
                },
            );
        }
    }
}

fn record_until_control(
    app: &AppHandle,
    state: &AppState,
    engine: &mut SpeechEngine,
    audio_receiver: Receiver<AudioChunk>,
    control_receiver: Receiver<SessionCommand>,
) -> RecordingResult {
    let started = Instant::now();
    let mut last_publish = started;
    let mut progress = TranscriptProgress::default();

    loop {
        match control_receiver.try_recv() {
            Ok(SessionCommand::Stop) => return RecordingResult::Stop(progress),
            Ok(SessionCommand::Cancel) => return RecordingResult::Cancel,
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                return RecordingResult::Failure {
                    progress,
                    error: "Recording control channel closed".to_owned(),
                };
            }
        }

        match audio_receiver.recv_timeout(CONTROL_POLL_INTERVAL) {
            Ok(AudioChunk {
                samples,
                input_level,
            }) => {
                progress.elapsed_ms = elapsed_ms(started);
                progress.input_level = input_level.clamp(0.0, 1.0);
                match engine.push(&samples) {
                    Ok(Some(revision)) => apply_engine_revision(&mut progress, revision),
                    Ok(None) => progress.revision = progress.revision.saturating_add(1),
                    Err(error) => {
                        return RecordingResult::Failure {
                            progress,
                            error: format!("Speech engine: {error}"),
                        };
                    }
                }
                if let Err(error) = publish_progress(app, state, &progress, false) {
                    return RecordingResult::Failure { progress, error };
                }
                last_publish = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                progress.elapsed_ms = elapsed_ms(started);
                if last_publish.elapsed() >= Duration::from_millis(100) {
                    progress.input_level *= 0.75;
                    if let Err(error) = publish_progress(app, state, &progress, false) {
                        return RecordingResult::Failure { progress, error };
                    }
                    last_publish = Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return RecordingResult::Failure {
                    progress,
                    error: "Microphone capture stopped".to_owned(),
                };
            }
        }
    }
}

fn finish_stop(
    app: &AppHandle,
    state: &AppState,
    engine: &mut SpeechEngine,
    mut progress: TranscriptProgress,
) -> StopCompletion {
    set_phase(state, AppPhase::Finalizing);
    progress.input_level = 0.0;

    let mut errors = Vec::new();
    let finalization_failed = match engine.finish() {
        Ok(revision) => {
            apply_engine_revision(&mut progress, revision);
            if let Err(error) = publish_progress(app, state, &progress, true) {
                errors.push(error);
            }
            false
        }
        Err(error) => {
            errors.push(format!("Finalize transcript: {error}"));
            true
        }
    };

    StopCompletion {
        progress,
        errors,
        finalization_failed,
    }
}

fn finish_cancel(app: &AppHandle, state: &AppState, target_window: ForegroundWindow) {
    let mut errors = Vec::new();
    if let Err(error) = hide_overlay(app) {
        errors.push(error);
    }
    if let Err(error) = restore_target_focus(target_window) {
        errors.push(error);
    }

    if errors.is_empty() {
        *state.phase.lock() = AppPhase::Ready;
        *state.transcript.lock() = TranscriptState::default();
    } else {
        clear_transcript_error(state, errors.join("; "));
    }
}

fn finish_stop_after_unload(
    app: &AppHandle,
    state: &AppState,
    target_window: ForegroundWindow,
    settings: SessionSettings,
    mut completion: StopCompletion,
) {
    if let Err(error) = hide_overlay(app) {
        completion.errors.push(error);
    }
    if let Err(error) = restore_target_focus(target_window) {
        completion.errors.push(error);
    }

    let text = transcript_text(&completion.progress);
    if !text.trim().is_empty() {
        let mut insertion_failed = false;
        if !completion.finalization_failed {
            if let Err(error) = insert_transcript(settings.insertion_method, &text) {
                insertion_failed = true;
                completion.errors.push(error);
            }
        }
        if let Err(error) = store_transcript(
            state,
            &text,
            &settings.model_name,
            settings.history_limit,
            insertion_failed,
        ) {
            completion.errors.push(error);
        }
    }

    if completion.errors.is_empty() {
        set_ready(state, completion.progress);
    } else {
        set_error_with_progress(state, completion.progress, completion.errors.join("; "));
    }
}

fn finish_failure(
    app: &AppHandle,
    state: &AppState,
    target_window: ForegroundWindow,
    settings: SessionSettings,
    mut completion: StopCompletion,
) {
    if let Err(error) = hide_overlay(app) {
        completion.errors.push(error);
    }
    if let Err(error) = restore_target_focus(target_window) {
        completion.errors.push(error);
    }

    let text = transcript_text(&completion.progress);
    if !text.trim().is_empty() {
        if let Err(error) = store_transcript(
            state,
            &text,
            &settings.model_name,
            settings.history_limit,
            false,
        ) {
            completion.errors.push(error);
        }
    }
    set_error_with_progress(state, completion.progress, completion.errors.join("; "));
}

fn insert_transcript(method: InsertionMethod, text: &str) -> Result<(), String> {
    match method {
        InsertionMethod::ClipboardReceipt => paste_preserving_clipboard(text.to_owned())
            .map_err(|error| format!("Insert transcript with clipboard receipt: {error}")),
        InsertionMethod::Typing => {
            type_text(text).map_err(|error| format!("Type transcript: {error}"))
        }
    }
}

fn store_transcript(
    state: &AppState,
    text: &str,
    model_name: &str,
    history_limit: u8,
    insertion_failed: bool,
) -> Result<(), String> {
    let session = TranscriptSession::completed(
        text,
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        model_name,
        insertion_failed,
    );
    state
        .history_store
        .record_session(&session, history_limit)
        .map_err(|error| format!("Store transcript: {error}"))?;
    Ok(())
}

fn apply_engine_revision(progress: &mut TranscriptProgress, revision: TranscriptRevision) {
    progress.revision = progress.revision.saturating_add(1).max(revision.revision);
    progress.stable_text = revision.stable_text;
    progress.revising_text = revision.revising_text;
}

fn publish_progress(
    app: &AppHandle,
    state: &AppState,
    progress: &TranscriptProgress,
    is_final: bool,
) -> Result<(), String> {
    *state.transcript.lock() = TranscriptState {
        stable_text: progress.stable_text.clone(),
        revising_text: progress.revising_text.clone(),
        elapsed_ms: progress.elapsed_ms,
        input_level: progress.input_level,
        error: None,
    };
    app.emit(
        TRANSCRIPT_REVISION_EVENT,
        TranscriptRevisionEvent {
            revision: progress.revision,
            stable_text: progress.stable_text.clone(),
            revising_text: progress.revising_text.clone(),
            is_final,
            elapsed_ms: progress.elapsed_ms,
            input_level: progress.input_level,
        },
    )
    .map_err(|error| format!("Emit transcript revision: {error}"))
}

fn publish_phase(app: &AppHandle, state: &AppState, phase: AppPhase) -> Result<(), String> {
    set_phase(state, phase);
    app.emit(SESSION_PHASE_EVENT, SessionPhaseEvent { phase })
        .map_err(|error| format!("Emit session phase: {error}"))
}

fn reset_for_start(state: &AppState) {
    set_phase(state, AppPhase::Loading);
    *state.transcript.lock() = TranscriptState::default();
}

fn set_phase(state: &AppState, phase: AppPhase) {
    *state.phase.lock() = phase;
}

fn set_ready(state: &AppState, mut progress: TranscriptProgress) {
    progress.input_level = 0.0;
    *state.phase.lock() = AppPhase::Ready;
    *state.transcript.lock() = TranscriptState {
        stable_text: progress.stable_text,
        revising_text: progress.revising_text,
        elapsed_ms: progress.elapsed_ms,
        input_level: progress.input_level,
        error: None,
    };
}

fn set_error_with_progress(state: &AppState, mut progress: TranscriptProgress, error: String) {
    progress.input_level = 0.0;
    *state.phase.lock() = AppPhase::Error;
    *state.transcript.lock() = TranscriptState {
        stable_text: progress.stable_text,
        revising_text: progress.revising_text,
        elapsed_ms: progress.elapsed_ms,
        input_level: progress.input_level,
        error: Some(error),
    };
}

fn clear_transcript_error(state: &AppState, error: String) {
    let mut transcript = state.transcript.lock();
    transcript.input_level = 0.0;
    transcript.error = Some(error);
    *state.phase.lock() = AppPhase::Error;
}

fn expose_error(state: &AppState, error: String) {
    state.transcript.lock().error = Some(error);
    *state.phase.lock() = AppPhase::Error;
}

fn fail_to_start(
    state: &AppState,
    sessions: &SessionManager,
    session_id: u64,
    error: String,
) -> Result<(), String> {
    sessions.clear(session_id);
    expose_error(state, error.clone());
    Err(error)
}

fn transcript_text(progress: &TranscriptProgress) -> String {
    let mut text = progress.stable_text.clone();
    if !progress.revising_text.trim().is_empty() {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(progress.revising_text.trim());
    }
    text.trim().to_owned()
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn show_overlay(app: &AppHandle) -> Result<(), String> {
    let overlay = app
        .get_webview_window(OVERLAY_WINDOW_LABEL)
        .ok_or_else(|| "Recording overlay window is not available".to_owned())?;
    overlay
        .show()
        .map_err(|error| format!("Show recording overlay: {error}"))
}

fn hide_overlay(app: &AppHandle) -> Result<(), String> {
    let overlay = app
        .get_webview_window(OVERLAY_WINDOW_LABEL)
        .ok_or_else(|| "Recording overlay window is not available".to_owned())?;
    overlay
        .hide()
        .map_err(|error| format!("Hide recording overlay: {error}"))
}

#[cfg(windows)]
fn current_foreground_window() -> Result<ForegroundWindow, String> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let window = unsafe { GetForegroundWindow() };
    if window.0.is_null() {
        return Err("No foreground target window is available".to_owned());
    }
    Ok(ForegroundWindow(window.0 as isize))
}

#[cfg(not(windows))]
fn current_foreground_window() -> Result<ForegroundWindow, String> {
    Err("Foreground window capture is available on Windows only".to_owned())
}

#[cfg(windows)]
fn restore_target_focus(target_window: ForegroundWindow) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;

    let window = HWND(target_window.0 as *mut std::ffi::c_void);
    if unsafe { SetForegroundWindow(window).as_bool() } {
        Ok(())
    } else {
        Err("Restore target window focus".to_owned())
    }
}

#[cfg(not(windows))]
fn restore_target_focus(_target_window: ForegroundWindow) -> Result<(), String> {
    Err("Target window focus restoration is available on Windows only".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn manager_rejects_concurrent_reservations_and_releases_them() {
        let manager = SessionManager::new();
        let first = manager.reserve().expect("reserve first session");
        assert!(manager.reserve().is_err());

        manager.clear(first.id);
        assert!(manager.reserve().is_ok());
    }

    #[test]
    fn stop_and_cancel_are_idempotent_when_control_queue_is_full_or_closed() {
        let manager = SessionManager::new();
        let reservation = manager.reserve().expect("reserve session");

        stop_session(&manager).expect("queue stop");
        stop_session(&manager).expect("repeat stop");
        assert_eq!(
            reservation.control_receiver.try_recv(),
            Ok(SessionCommand::Stop)
        );
        assert_eq!(
            reservation.control_receiver.try_recv(),
            Err(TryRecvError::Empty)
        );

        drop(reservation.control_receiver);
        cancel_session(&manager).expect("cancel after receiver closes");
    }

    #[test]
    fn event_serialization_matches_frontend_transcript_contract() {
        let event = TranscriptRevisionEvent {
            revision: 7,
            stable_text: "stable".to_owned(),
            revising_text: "tail".to_owned(),
            is_final: true,
            elapsed_ms: 1250,
            input_level: 0.4,
        };

        assert_eq!(
            serde_json::to_value(event).expect("serialize transcript event"),
            json!({
                "revision": 7,
                "stableText": "stable",
                "revisingText": "tail",
                "final": true,
                "elapsedMs": 1250,
                "inputLevel": 0.4_f32,
            })
        );
    }

    #[test]
    fn phase_event_serialization_matches_frontend_contract() {
        assert_eq!(
            serde_json::to_value(SessionPhaseEvent {
                phase: AppPhase::Loading,
            })
            .expect("serialize phase event"),
            json!({ "phase": "loading" })
        );
    }

    #[test]
    fn transcript_text_commits_the_unstable_tail() {
        let progress = TranscriptProgress {
            stable_text: "a stable sentence".to_owned(),
            revising_text: "and its tail".to_owned(),
            ..TranscriptProgress::default()
        };
        assert_eq!(transcript_text(&progress), "a stable sentence and its tail");
    }
}
