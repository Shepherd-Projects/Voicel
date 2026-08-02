# Voicel architecture

Voicel is a Tauri 2 application with a React presentation layer and a Windows-focused Rust core.

## Runtime boundaries

- `audio`: CPAL capture, channel conversion, and resampling to 16 kHz mono `f32` frames.
- `engine`: a common revisioned transcript stream over sherpa-onnx online and offline recognizers.
- `session`: the recording state machine, cancellation, stable/final text, and event delivery.
- `insertion`: literal typing and receipt-sequenced Windows clipboard insertion.
- `settings`: atomic, versioned JSON below Tauri's per-user app-data directory.
- `history`: a bounded one-to-three entry store next to settings.
- `platform`: global shortcuts, non-activating overlay, tray, startup, and focus restoration.
- `ui`: typed commands/events only; it does not own audio, model, or persistence state.

## Transcript revisions

Every engine update carries a monotonically increasing revision, stable text, revising text, and a final flag. The UI replaces only the revising range. Online Zipformer advances its decoder state per audio chunk. Parakeet v3 receives bounded overlapping windows; consecutive hypotheses are normalized and their longest trustworthy common prefix is promoted to stable text. Stop flushes the final tail exactly once.

## Persistence

The executable and bundled assets are immutable. User state is stored under `%LOCALAPPDATA%/com.voicel.desktop/` using write-to-sibling, flush, and atomic replace. Settings include `schema_version`; migrations preserve known fields and default only fields absent from older schemas. Invalid files are renamed with a recovery timestamp instead of overwritten.

## Clipboard transaction

Clipboard insertion snapshots all registered formats, advertises the transcript using delayed rendering, sends `Ctrl+V`, waits for Windows to request the promised Unicode text, and then restores the snapshot. A bounded timeout exposes a recoverable failure and leaves the transcript copyable; it never reports success merely because the keystroke was queued.

## Update boundary

Installers may replace the app directory only. They do not own model files, settings, history, or logs. Future updates must migrate app-data schemas in place and cannot change the application identifier.
