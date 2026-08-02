# Voicel product contract

Voicel is a local, keyboard-first Windows dictation utility. Its primary job is to make spoken text appear in the previously focused text field with visible progress while the user is still speaking.

## Interaction contract

- `Toggle recording` starts and stops a session. Push-to-talk is not part of the product.
- `Cancel` discards the active session without inserting text or adding history.
- Stopping commits the final unstable tail, inserts the resulting text, stores the transcript, and returns focus to the target application.
- Clipboard insertion must restore every clipboard format after the target application has consumed the temporary transcript. Literal typing remains available for incompatible applications.
- The compact always-on-top overlay shows recording state, input level, elapsed time, stable text, changing text, stop, and cancel. It never steals focus from the target application.
- The app can launch at sign-in, start hidden, live in the tray, and unload its model immediately after each session.
- Settings and the last one to three transcripts are stored below the user's application-data directory with versioned migrations. Updating or replacing the executable cannot reset them.

## Model modes

- **Live English** uses an online Zipformer transducer through sherpa-onnx. Audio is decoded as it arrives, so this is genuinely stateful streaming and is the default low-latency route.
- **Parakeet v3** retains the user's preferred accurate multilingual model. The available ONNX artifact is offline-only, so Voicel repeatedly decodes bounded overlapping windows and stabilizes their common prefix. The UI calls this `Incremental`, not `Streaming`.
- A model capability is shown from catalog metadata; Voicel never implies an offline model is stateful streaming.

## Surface obligations

| Surface | Immediate | Persistent | On demand |
| --- | --- | --- | --- |
| Live | readiness, selected model, shortcut, transcript, record/stop/cancel, model/download errors | stable versus revising text, elapsed time, input level | input device and latency detail |
| History | newest transcript and copy action | timestamp and model provenance | older two entries and clear action |
| Words | add a word, existing words, validation | explanation of engine support | import/export |
| Models | installed/active state and capability truth | size, languages, streaming class | download, remove, load/unload detail |
| Settings | shortcuts and insertion behavior | startup, hidden, tray, unload, history limit | diagnostics and data location |

## State and recovery

The product distinguishes `missing model`, `downloading`, `loading`, `ready`, `recording`, `finalizing`, `inserting`, `cancelled`, and `error`. Recording controls are available only in states where they have an honest effect. Download, microphone, model-load, and insertion failures retain the transcript and expose a specific retry or copy action.

## Visual direction

The main window behaves like a compact listening instrument rather than a dashboard: one continuous work surface, a strong live transcript field, and a narrow command/status rail. Density comes from simultaneous operational information, not repeated cards. Motion explains recording and text revision, stops under reduced-motion preferences, and never substitutes for state labels.

## Acceptance evidence

- A long recording produces visible partial text before stop and only finalizes its tail afterward.
- Cancel inserts and stores nothing.
- Rich clipboard content survives a clipboard-based insertion unchanged.
- Settings and history survive executable replacement and schema migration.
- Global shortcuts work while the main window is hidden.
- The overlay remains non-activating and all main-window actions are keyboard reachable with visible focus.
- Model capability labels match the runtime used.
