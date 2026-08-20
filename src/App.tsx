import {
  Activity,
  BookOpen,
  Check,
  ChevronRight,
  CircleAlert,
  CircleCheck,
  Clipboard,
  Copy,
  Cpu,
  Download,
  History as HistoryIcon,
  Keyboard,
  Loader2,
  Mic2,
  Minus,
  Plus,
  RefreshCw,
  Save,
  Settings2,
  SlidersHorizontal,
  Square,
  Trash2,
  Volume2,
  X,
  Zap,
  type LucideIcon,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  type AppSettings,
  type AppSnapshot,
  type HistoryItem,
  type ModelInfo,
  type RuntimeMode,
  type WordEntry,
  createVoicelAdapter,
} from "./lib/adapter";
import "./App.css";

type ViewId = "live" | "history" | "words" | "models" | "settings";
type LoadState = "loading" | "ready" | "error";
type SessionStatus =
  | "idle"
  | "starting"
  | "recording"
  | "stopping"
  | "completed"
  | "cancelling"
  | "error";

interface SessionUi {
  status: SessionStatus;
  committedText: string;
  liveText: string;
  level: number;
  elapsedSeconds: number;
  error?: string;
}
interface NoticeState {
  tone: "success" | "info" | "error";
  title: string;
  body: string;
}
interface ModelOperation {
  kind: "install" | "select";
  id: string;
  error?: string;
}

const EMPTY_SESSION: SessionUi = {
  status: "idle",
  committedText: "",
  liveText: "",
  level: 0,
  elapsedSeconds: 0,
};
const NAV_ITEMS: Array<{ id: ViewId; label: string; icon: LucideIcon }> = [
  { id: "live", label: "Live", icon: Mic2 },
  { id: "history", label: "History", icon: HistoryIcon },
  { id: "words", label: "Words", icon: BookOpen },
  { id: "models", label: "Models", icon: Cpu },
  { id: "settings", label: "Settings", icon: SlidersHorizontal },
];
const VIEW_COPY: Record<ViewId, { title: string; description: string }> = {
  live: { title: "Live", description: "Dictate into whichever app has focus." },
  history: {
    title: "History",
    description: "Your latest finished transcripts.",
  },
  words: {
    title: "Words",
    description: "Correct names and specialist terms.",
  },
  models: {
    title: "Models",
    description: "Choose streaming speed or multilingual accuracy.",
  },
  settings: {
    title: "Settings",
    description: "Startup, shortcuts, and text delivery.",
  },
};

function errorMessage(error: unknown): string {
  return error instanceof Error
    ? error.message
    : "Something went wrong. Try again.";
}
function formatDuration(seconds: number): string {
  const minutes = Math.floor(seconds / 60);
  return `${String(minutes).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`;
}
function formatHistoryDate(value: string): string {
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  }).format(new Date(value));
}
function joinTranscript(committedText: string, liveText: string): string {
  return [committedText.trim(), liveText.trim()]
    .filter(Boolean)
    .join(" ")
    .replace(/\s+/g, " ");
}
function makeHistoryId(): string {
  return `history-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

function matchesShortcut(event: KeyboardEvent, shortcut: string): boolean {
  const parts = shortcut.toLowerCase().replace(/\s+/g, "").split("+");
  const key = parts[parts.length - 1];
  const eventKey = event.code === "Space" ? "space" : event.key.toLowerCase();
  return (
    Boolean(key) &&
    (key === eventKey || (key === "esc" && eventKey === "escape")) &&
    event.ctrlKey === parts.includes("ctrl") &&
    event.shiftKey === parts.includes("shift") &&
    event.altKey === parts.includes("alt") &&
    event.metaKey === parts.includes("meta")
  );
}
function findSelectedModel(snapshot: AppSnapshot | null): ModelInfo | undefined {
  return snapshot?.models.find(
    (model) => model.id === snapshot.health.selectedModelId,
  );
}

export default function App() {
  const adapter = useMemo(() => createVoicelAdapter(), []);
  const [activeView, setActiveView] = useState<ViewId>("live");
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [loadError, setLoadError] = useState("");
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [session, setSession] = useState<SessionUi>(EMPTY_SESSION);
  const [notice, setNotice] = useState<NoticeState | null>(null);
  const [copiedHistoryId, setCopiedHistoryId] = useState<string | null>(null);
  const [historyClearing, setHistoryClearing] = useState(false);
  const [modelOperation, setModelOperation] = useState<ModelOperation | null>(
    null,
  );
  const [settingsSaving, setSettingsSaving] = useState(false);
  const selectedModel = findSelectedModel(snapshot);
  const canRecord = Boolean(selectedModel?.installed);
  const sessionRef = useRef(session);
  const snapshotRef = useRef(snapshot);
  useLayoutEffect(() => {
    sessionRef.current = session;
    snapshotRef.current = snapshot;
  }, [session, snapshot]);

  const showNotice = useCallback((next: NoticeState) => setNotice(next), []);
  useEffect(() => {
    if (!notice) return undefined;
    const id = window.setTimeout(() => setNotice(null), 5000);
    return () => window.clearTimeout(id);
  }, [notice]);
  const refreshSnapshot = useCallback(async () => {
    setLoadState("loading");
    setLoadError("");
    try {
      setSnapshot(await adapter.loadSnapshot());
      setLoadState("ready");
    } catch (error) {
      setLoadState("error");
      setLoadError(errorMessage(error));
    }
  }, [adapter]);
  useEffect(() => {
    void refreshSnapshot();
  }, [refreshSnapshot]);

  useEffect(() => {
    if (activeView !== "history") return undefined;
    let cancelled = false;
    const syncHistory = () => {
      void adapter
        .loadSnapshot()
        .then((next) => {
          if (!cancelled) setSnapshot(next);
        })
        .catch((error: unknown) => {
          if (!cancelled) {
            showNotice({
              tone: "error",
              title: "Could not refresh History",
              body: errorMessage(error),
            });
          }
        });
    };
    syncHistory();
    window.addEventListener("focus", syncHistory);
    return () => {
      cancelled = true;
      window.removeEventListener("focus", syncHistory);
    };
  }, [activeView, adapter, showNotice]);

  const startRecording = useCallback(async () => {
    const currentSnapshot = snapshotRef.current;
    const currentSelectedModel = findSelectedModel(currentSnapshot);
    if (!currentSelectedModel?.installed) {
      showNotice({
        tone: "info",
        title: "Choose a model first",
        body: "Open Models and select an installed model before starting a session.",
      });
      setActiveView("models");
      return;
    }
    if (["starting", "recording", "stopping"].includes(sessionRef.current.status))
      return;
    setSession({ ...EMPTY_SESSION, status: "starting" });
    try {
      await adapter.startRecording();
      setSession({ ...EMPTY_SESSION, status: "recording" });
    } catch (error) {
      const message = errorMessage(error);
      setSession({ ...EMPTY_SESSION, status: "error", error: message });
      showNotice({
        tone: "error",
        title: "Could not start listening",
        body: message,
      });
    }
  }, [adapter, showNotice]);

  useEffect(() => {
    if (session.status !== "recording") return undefined;
    let cancelled = false;
    let unsubscribe: (() => void) | undefined;
    void adapter
      .subscribeToSession((event) => {
        if (cancelled) return;
        setSession((current) =>
          current.status !== "recording"
            ? current
            : {
                ...current,
                committedText: event.committedText ?? current.committedText,
                liveText: event.liveText ?? current.liveText,
                level: event.level ?? current.level,
                elapsedSeconds: event.elapsedSeconds ?? current.elapsedSeconds,
                error: event.error,
              },
        );
      })
      .then((cleanup) => {
        if (cancelled) cleanup();
        else unsubscribe = cleanup;
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          const message = errorMessage(error);
          setSession((current) => ({
            ...current,
            status: "error",
            error: message,
          }));
          showNotice({
            tone: "error",
            title: "Listening stream stopped",
            body: message,
          });
        }
      });
    return () => {
      cancelled = true;
      unsubscribe?.();
    };
  }, [adapter, session.status, showNotice]);

  const stopRecording = useCallback(async () => {
    const previous = sessionRef.current;
    if (previous.status !== "recording") return;
    const currentSnapshot = snapshotRef.current;
    const selectedModelName = findSelectedModel(currentSnapshot)?.name;
    setSession((current) => ({ ...current, status: "stopping" }));
    try {
      const finalEvent = await adapter.stopRecording();
      const committedText = finalEvent?.committedText ?? previous.committedText;
      const liveText = finalEvent?.liveText ?? previous.liveText;
      const text = joinTranscript(committedText, liveText);
      if (text) {
        const item: HistoryItem = {
          id: makeHistoryId(),
          text,
          createdAt: new Date().toISOString(),
          durationSeconds: Math.max(1, previous.elapsedSeconds),
          modelName: selectedModelName ?? "Unknown model",
          source: adapter.mode,
        };
        await adapter.addHistory(item);
        setSnapshot((current) =>
          current
            ? {
                ...current,
                history: [item, ...current.history].slice(
                  0,
                  current.settings.historyLimit,
                ),
              }
            : current,
        );
      }
      setSession({
        ...EMPTY_SESSION,
        status: "completed",
        committedText,
        liveText,
        elapsedSeconds: previous.elapsedSeconds,
      });
      showNotice({
        tone: "success",
        title: text ? "Transcript committed" : "Session ended",
        body: text
          ? "Saved to History. The live words are now committed."
          : "No words were captured.",
      });
    } catch (error) {
      const message = errorMessage(error);
      setSession({ ...previous, status: "error", error: message });
      showNotice({
        tone: "error",
        title: "Could not finish the transcript",
        body: message,
      });
    }
  }, [adapter, showNotice]);

  const cancelRecording = useCallback(async () => {
    const previous = sessionRef.current;
    if (previous.status !== "recording" && previous.status !== "error") return;
    setSession((current) => ({ ...current, status: "cancelling" }));
    try {
      await adapter.cancelRecording();
      setSession(EMPTY_SESSION);
      showNotice({
        tone: "info",
        title: "Session cancelled",
        body: "Nothing from this session was added to History.",
      });
    } catch (error) {
      const message = errorMessage(error);
      setSession({ ...previous, status: "error", error: message });
      showNotice({
        tone: "error",
        title: "Could not cancel the session",
        body: message,
      });
    }
  }, [adapter, showNotice]);
  const retryRecording = useCallback(() => {
    setSession(EMPTY_SESSION);
    void startRecording();
  }, [startRecording]);

  useEffect(() => {
    if (adapter.mode !== "demo") return undefined;
    const onKeyDown = (event: KeyboardEvent) => {
      const currentSnapshot = snapshotRef.current;
      if (!currentSnapshot) return;
      const target = event.target as HTMLElement | null;
      const editing = Boolean(
        target &&
          (["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName) ||
            target.isContentEditable ||
            target.closest("[data-shortcut-recorder]")),
      );
      if (
        !editing &&
        !event.repeat &&
        matchesShortcut(event, currentSnapshot.settings.hotkey)
      ) {
        event.preventDefault();
        if (sessionRef.current.status === "recording") void stopRecording();
        else void startRecording();
      }
      if (
        !editing &&
        !event.repeat &&
        matchesShortcut(event, currentSnapshot.settings.cancelHotkey) &&
        (sessionRef.current.status === "recording" ||
          sessionRef.current.status === "error")
      ) {
        event.preventDefault();
        void cancelRecording();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [
    adapter.mode,
    cancelRecording,
    startRecording,
    stopRecording,
  ]);

  const copyText = useCallback(
    async (text: string, historyId?: string) => {
      try {
        await adapter.copyText(text);
        if (historyId) {
          setCopiedHistoryId(historyId);
          window.setTimeout(() => setCopiedHistoryId(null), 1800);
        }
        showNotice({
          tone: "success",
          title: "Copied",
          body: "The committed transcript is ready to paste.",
        });
      } catch (error) {
        showNotice({
          tone: "error",
          title: "Copy failed",
          body: errorMessage(error),
        });
      }
    },
    [adapter, showNotice],
  );
  const clearHistory = useCallback(async () => {
    setHistoryClearing(true);
    try {
      await adapter.clearHistory();
      setSnapshot((current) =>
        current ? { ...current, history: [] } : current,
      );
      showNotice({
        tone: "success",
        title: "History cleared",
        body: "New transcripts will still appear here after you commit them.",
      });
    } catch (error) {
      showNotice({
        tone: "error",
        title: "Could not clear History",
        body: errorMessage(error),
      });
    } finally {
      setHistoryClearing(false);
    }
  }, [adapter, showNotice]);
  const addWord = useCallback(
    async (text: string) => {
      const word = await adapter.addWord(text);
      setSnapshot((current) =>
        current ? { ...current, words: [word, ...current.words] } : current,
      );
    },
    [adapter],
  );
  const updateWord = useCallback(
    async (id: string, text: string) => {
      const word = await adapter.updateWord(id, text);
      setSnapshot((current) =>
        current
          ? {
              ...current,
              words: current.words.map((entry) =>
                entry.id === id ? word : entry,
              ),
            }
          : current,
      );
    },
    [adapter],
  );
  const deleteWord = useCallback(
    async (id: string) => {
      await adapter.deleteWord(id);
      setSnapshot((current) =>
        current
          ? {
              ...current,
              words: current.words.filter((word) => word.id !== id),
            }
          : current,
      );
    },
    [adapter],
  );
  const installModel = useCallback(
    async (id: string) => {
      setModelOperation({ kind: "install", id });
      try {
        const model = await adapter.installModel(id);
        setSnapshot((current) =>
          current
            ? {
                ...current,
                models: current.models.map((entry) =>
                  entry.id === id ? model : entry,
                ),
              }
            : current,
        );
        setModelOperation(null);
        showNotice({
          tone: "success",
          title: "Model installed",
          body: `${model.name} is ready to select.`,
        });
      } catch (error) {
        setModelOperation({ kind: "install", id, error: errorMessage(error) });
      }
    },
    [adapter, showNotice],
  );
  const selectModel = useCallback(
    async (id: string) => {
      setModelOperation({ kind: "select", id });
      try {
        await adapter.selectModel(id);
        setSnapshot((current) =>
          current
            ? { ...current, health: { ...current.health, selectedModelId: id } }
            : current,
        );
        setModelOperation(null);
        showNotice({
          tone: "success",
          title: "Model selected",
          body: "The next session will use this local model.",
        });
      } catch (error) {
        setModelOperation({ kind: "select", id, error: errorMessage(error) });
      }
    },
    [adapter, showNotice],
  );
  const saveSettings = useCallback(
    async (settings: AppSettings) => {
      setSettingsSaving(true);
      try {
        const saved = await adapter.saveSettings(settings);
        setSnapshot((current) =>
          current ? { ...current, settings: saved } : current,
        );
        showNotice({
          tone: "success",
          title: "Settings saved",
          body: "Your startup, hotkey, and delivery choices are updated.",
        });
      } finally {
        setSettingsSaving(false);
      }
    },
    [adapter, showNotice],
  );
  const setShortcutCapture = useCallback(
    (active: boolean) => adapter.setShortcutCapture(active),
    [adapter],
  );
  const minimizeWindow = () => {
    if (adapter.mode === "tauri") void getCurrentWindow().minimize();
  };
  const hideWindow = () => {
    if (adapter.mode === "tauri") void getCurrentWindow().hide();
  };

  return (
    <div className="app-shell">
      <aside className="app-rail" aria-label="Primary navigation">
        <div className="rail-brand">
          <div className="brand-mark" aria-hidden="true">
            <Activity size={17} strokeWidth={2.2} />
          </div>
          <div>
            <div className="brand-name">Voicel</div>
            <div className="brand-caption">Listening instrument</div>
          </div>
        </div>
        <nav className="nav-list" aria-label="Views">
          {NAV_ITEMS.map((item) => {
            const Icon = item.icon;
            const active = activeView === item.id;
            return (
              <button
                className={`nav-item${active ? " is-active" : ""}`}
                key={item.id}
                type="button"
                aria-current={active ? "page" : undefined}
                onClick={() => setActiveView(item.id)}
              >
                <Icon size={18} strokeWidth={1.85} aria-hidden="true" />
                <span>{item.label}</span>
                {active ? (
                  <ChevronRight size={15} strokeWidth={2} aria-hidden="true" />
                ) : null}
              </button>
            );
          })}
        </nav>
      </aside>
      <div className="app-main">
        <header className="topbar" data-tauri-drag-region>
          <div className="topbar-heading" data-tauri-drag-region>
            <h1 data-tauri-drag-region>{VIEW_COPY[activeView].title}</h1>
            <p data-tauri-drag-region>{VIEW_COPY[activeView].description}</p>
          </div>
          <div className="topbar-meta">
            <div className="window-controls" aria-label="Window controls">
              <button
                type="button"
                onClick={minimizeWindow}
                aria-label="Minimize Voicel"
              >
                <Minus size={14} strokeWidth={2} aria-hidden="true" />
              </button>
              <button
                type="button"
                onClick={hideWindow}
                aria-label="Hide Voicel to the tray"
              >
                <X size={14} strokeWidth={2} aria-hidden="true" />
              </button>
            </div>
            {adapter.mode === "demo" ? (
              <div className="runtime-tag demo">DEMO · NO NATIVE AUDIO</div>
            ) : null}
          </div>
        </header>
        {notice ? (
          <Notice notice={notice} onDismiss={() => setNotice(null)} />
        ) : null}
        <main className="content" id="main-content" tabIndex={-1}>
          {loadState === "loading" ? <LoadingView /> : null}
          {loadState === "error" ? (
            <ErrorView
              message={loadError}
              onRetry={() => void refreshSnapshot()}
            />
          ) : null}
          {loadState === "ready" && snapshot ? (
            <>
              {activeView === "live" ? (
                <LiveView
                  adapterMode={adapter.mode}
                  session={session}
                  selectedModel={selectedModel}
                  canRecord={canRecord}
                  hotkey={snapshot.settings.hotkey}
                  onStart={() => void startRecording()}
                  onStop={() => void stopRecording()}
                  onCancel={() => void cancelRecording()}
                  onRetry={() => void retryRecording()}
                  onCopy={(text) => void copyText(text)}
                  onGoToModels={() => setActiveView("models")}
                />
              ) : null}
              {activeView === "history" ? (
                <HistoryView
                  history={snapshot.history}
                  copiedId={copiedHistoryId}
                  clearing={historyClearing}
                  onCopy={(text, id) => void copyText(text, id)}
                  onClear={() => void clearHistory()}
                  onGoToLive={() => setActiveView("live")}
                />
              ) : null}
              {activeView === "words" ? (
                <WordsView
                  words={snapshot.words}
                  onAdd={addWord}
                  onUpdate={updateWord}
                  onDelete={deleteWord}
                />
              ) : null}
              {activeView === "models" ? (
                <ModelsView
                  models={snapshot.models}
                  selectedId={snapshot.health.selectedModelId}
                  selectedModel={selectedModel}
                  operation={modelOperation}
                  onInstall={(id) => void installModel(id)}
                  onSelect={(id) => void selectModel(id)}
                />
              ) : null}
              {activeView === "settings" ? (
                <SettingsView
                  settings={snapshot.settings}
                  saving={settingsSaving}
                  onSave={saveSettings}
                  onShortcutCapture={setShortcutCapture}
                />
              ) : null}
            </>
          ) : null}
        </main>
      </div>
    </div>
  );
}

function Notice({
  notice,
  onDismiss,
}: {
  notice: NoticeState;
  onDismiss: () => void;
}) {
  const Icon =
    notice.tone === "error"
      ? CircleAlert
      : notice.tone === "success"
        ? CircleCheck
        : Activity;
  return (
    <div
      className={`notice notice-${notice.tone}`}
      role={notice.tone === "error" ? "alert" : "status"}
    >
      <Icon size={17} strokeWidth={2} aria-hidden="true" />
      <div>
        <strong>{notice.title}</strong>
        <span>{notice.body}</span>
      </div>
      <button
        className="icon-button notice-close"
        type="button"
        aria-label="Dismiss notification"
        onClick={onDismiss}
      >
        <X size={16} strokeWidth={2} aria-hidden="true" />
      </button>
    </div>
  );
}
function LoadingView() {
  return (
    <section className="state-view" aria-live="polite" aria-busy="true">
      <Loader2
        className="spin"
        size={22}
        strokeWidth={1.8}
        aria-hidden="true"
      />
      <h2>Loading workspace</h2>
      <p>Checking the local model, history, and preferences.</p>
    </section>
  );
}
function ErrorView({
  message,
  onRetry,
}: {
  message: string;
  onRetry: () => void;
}) {
  return (
    <section className="state-view state-error" role="alert">
      <CircleAlert size={24} strokeWidth={1.8} aria-hidden="true" />
      <h2>Workspace unavailable</h2>
      <p>{message || "Voicel could not load its local workspace."}</p>
      <button className="button button-primary" type="button" onClick={onRetry}>
        <RefreshCw size={16} strokeWidth={2} aria-hidden="true" /> Retry
        workspace
      </button>
    </section>
  );
}

function LiveView({
  adapterMode,
  session,
  selectedModel,
  canRecord,
  hotkey,
  onStart,
  onStop,
  onCancel,
  onRetry,
  onCopy,
  onGoToModels,
}: {
  adapterMode: RuntimeMode;
  session: SessionUi;
  selectedModel?: ModelInfo;
  canRecord: boolean;
  hotkey: string;
  onStart: () => void;
  onStop: () => void;
  onCancel: () => void;
  onRetry: () => void;
  onCopy: (text: string) => void;
  onGoToModels: () => void;
}) {
  const recording = session.status === "recording";
  const working = ["starting", "stopping", "cancelling"].includes(
    session.status,
  );
  const transcript = joinTranscript(session.committedText, session.liveText);
  const hasTranscript = Boolean(transcript);
  const statusLabel = recording ? "Listening" : null;
  return (
    <section className="view live-view" aria-labelledby="live-heading">
      <div className="live-toolbar">
        <div className="live-status-line">
          {statusLabel ? (
            <>
              <div className="signal-state live" role="status" aria-live="polite">
                <span className="status-dot live" aria-hidden="true" />{" "}
                <span>{statusLabel}</span>
              </div>
              <span className="separator" aria-hidden="true" />
            </>
          ) : null}
          <span className="model-readout">
            {selectedModel?.name ?? "No model selected"}
          </span>
        </div>
        <button className="text-button" type="button" onClick={onGoToModels}>
          Change model{" "}
          <ChevronRight size={15} strokeWidth={2} aria-hidden="true" />
        </button>
      </div>
      <div className="live-grid">
        <section className="transcript-surface" aria-labelledby="live-heading">
          <div className="surface-heading">
            <h2 id="live-heading">Transcript</h2>
          </div>
          {session.status === "error" ? (
            <div className="inline-error" role="alert">
              <CircleAlert size={18} strokeWidth={2} aria-hidden="true" />
              <div>
                <strong>Listening could not continue.</strong>
                <span>
                  {session.error ?? "The native session ended unexpectedly."}
                </span>
              </div>
              <button
                className="button button-quiet"
                type="button"
                onClick={onRetry}
              >
                <RefreshCw size={15} strokeWidth={2} aria-hidden="true" /> Retry
              </button>
            </div>
          ) : null}
          <div
            className={`transcript-body ${recording ? "is-recording" : ""}`}
            aria-live="polite"
          >
            {session.status === "idle" ? (
              <div className="transcript-empty">
                <div className="empty-signal" aria-hidden="true">
                  <Mic2 size={24} strokeWidth={1.65} />
                </div>
                <p>
                  Press <kbd>{hotkey}</kbd> to start a session.
                </p>
              </div>
            ) : null}
            {session.status === "starting" ? (
              <div className="transcript-empty" aria-busy="true">
                <Loader2
                  className="spin"
                  size={25}
                  strokeWidth={1.7}
                  aria-hidden="true"
                />
                <p>Opening the listening session.</p>
                <span>Microphone and model are being connected.</span>
              </div>
            ) : null}
            {recording ? (
              <div className="transcript-stream">
                <div className="transcript-lane committed-lane">
                  <span className="lane-label">Committed</span>
                  <p>
                    {session.committedText ||
                      "Waiting for the first committed words…"}
                  </p>
                </div>
                <div className="transcript-lane live-lane">
                  <span className="lane-label">Live</span>
                  <p>{session.liveText || "Listening…"}</p>
                </div>
              </div>
            ) : null}
            {session.status === "stopping" ||
            session.status === "cancelling" ? (
              <div className="transcript-empty" aria-busy="true">
                <Loader2
                  className="spin"
                  size={25}
                  strokeWidth={1.7}
                  aria-hidden="true"
                />
                <p>
                  {session.status === "stopping"
                    ? "Committing the last words."
                    : "Cancelling this session."}
                </p>
                <span>
                  {session.status === "stopping"
                    ? "The live line will be folded into History."
                    : "Nothing will be added to History."}
                </span>
              </div>
            ) : null}
            {session.status === "completed" ? (
              <div className="transcript-stream completed-stream">
                <div className="transcript-lane committed-lane">
                  <span className="lane-label">
                    <Check size={13} strokeWidth={2.4} aria-hidden="true" />{" "}
                    Committed
                  </span>
                  <p>
                    {hasTranscript ? transcript : "No words were captured."}
                  </p>
                </div>
                <div className="commit-confirmation">
                  <CircleCheck size={15} strokeWidth={2} aria-hidden="true" />{" "}
                  Saved to History
                </div>
              </div>
            ) : null}
            {session.status === "error" && hasTranscript ? (
              <div className="transcript-stream preserved-stream">
                <div className="transcript-lane committed-lane">
                  <span className="lane-label">Captured so far</span>
                  <p>{transcript}</p>
                </div>
              </div>
            ) : null}
          </div>
          <div className="transcript-footer">
            {recording || hasTranscript ? (
              <div
                className="transcript-legend"
                aria-label="Transcript line legend"
              >
                <span>
                  <i
                    className="legend-mark committed-mark"
                    aria-hidden="true"
                  />{" "}
                  Committed text
                </span>
                <span>
                  <i className="legend-mark live-mark" aria-hidden="true" />{" "}
                  Live text
                </span>
              </div>
            ) : null}
            <div className="transcript-actions">
              {session.status === "completed" && transcript ? (
                <button
                  className="button button-quiet"
                  type="button"
                  onClick={() => onCopy(transcript)}
                >
                  <Copy size={15} strokeWidth={2} aria-hidden="true" /> Copy
                  transcript
                </button>
              ) : null}
              {recording ? (
                <>
                  <button
                    className="button button-danger"
                    type="button"
                    onClick={onCancel}
                  >
                    <X size={15} strokeWidth={2} aria-hidden="true" /> Cancel
                  </button>
                  <button
                    className="button button-primary"
                    type="button"
                    onClick={onStop}
                  >
                    <Square
                      size={14}
                      strokeWidth={2.3}
                      fill="currentColor"
                      aria-hidden="true"
                    />{" "}
                    Stop &amp; commit
                  </button>
                </>
              ) : null}
              {!recording &&
              session.status !== "completed" &&
              session.status !== "error" ? (
                <button
                  className="button button-primary start-button"
                  type="button"
                  disabled={!canRecord || working}
                  onClick={onStart}
                >
                  {working ? (
                    <Loader2
                      className="spin"
                      size={16}
                      strokeWidth={2}
                      aria-hidden="true"
                    />
                  ) : (
                    <Mic2 size={16} strokeWidth={2} aria-hidden="true" />
                  )}
                  {session.status === "starting"
                    ? "Starting…"
                    : "Start listening"}
                </button>
              ) : null}
              {session.status === "completed" ? (
                <button
                  className="button button-primary"
                  type="button"
                  onClick={onStart}
                  disabled={!canRecord}
                >
                  <Mic2 size={16} strokeWidth={2} aria-hidden="true" /> Start
                  another
                </button>
              ) : null}
            </div>
          </div>
          {!canRecord ? (
            <div className="availability-note">
              <CircleAlert size={15} strokeWidth={2} aria-hidden="true" />
              <span>No installed model is selected.</span>
              <button
                className="text-button"
                type="button"
                onClick={onGoToModels}
              >
                Open Models
              </button>
            </div>
          ) : null}
        </section>
        <aside className="live-inspector" aria-label="Listening status">
          <section
            className="inspector-section signal-section"
            aria-labelledby="signal-heading"
          >
            <div className="section-topline">
              <div className="eyebrow" id="signal-heading">
                Input signal
              </div>
              <Volume2 size={19} strokeWidth={1.7} aria-hidden="true" />
            </div>
            <SignalMeter
              level={session.level}
              active={recording}
              adapterMode={adapterMode}
            />
            <div className="signal-reading">
              <span>
                {adapterMode === "demo" ? "Preview level" : "Input level"}
              </span>
              <strong>{Math.round(session.level * 100)}%</strong>
            </div>
          </section>
          <section
            className="inspector-section time-section"
            aria-labelledby="time-heading"
          >
            <div className="eyebrow">Session time</div>
            <div className="session-time" id="time-heading">
              {formatDuration(session.elapsedSeconds)}
            </div>
          </section>
          {adapterMode === "tauri" ? (
            <div className="inspector-footnote">
              <Zap size={14} strokeWidth={1.8} aria-hidden="true" />
              <span>Native audio stays on this device.</span>
            </div>
          ) : null}
        </aside>
      </div>
    </section>
  );
}

function SignalMeter({
  level,
  active,
  adapterMode,
}: {
  level: number;
  active: boolean;
  adapterMode: RuntimeMode;
}) {
  const pattern = [
    0.28, 0.54, 0.4, 0.76, 0.5, 0.88, 0.36, 0.65, 0.44, 0.72, 0.32, 0.56, 0.42,
    0.7, 0.3, 0.48,
  ];
  const normalized = Math.max(0, Math.min(1, level));
  return (
    <div className="meter-wrap">
      <div
        className={`signal-meter ${active ? "active" : ""}`}
        role="meter"
        aria-label={`${adapterMode === "demo" ? "Preview" : "Input"} level`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(normalized * 100)}
      >
        {pattern.map((height, index) => (
          <span
            className="signal-bar"
            key={index}
            style={{
              height:
                active && normalized > 0.01
                  ? `${Math.min(100, Math.max(8, Math.round(height * (0.35 + normalized * 0.75) * 100)))}%`
                  : "3px",
            }}
            aria-hidden="true"
          />
        ))}
      </div>
      <div className="meter-scale" aria-hidden="true">
        <span>quiet</span>
        <span>clear</span>
        <span>peak</span>
      </div>
    </div>
  );
}

function HistoryView({
  history,
  copiedId,
  clearing,
  onCopy,
  onClear,
  onGoToLive,
}: {
  history: HistoryItem[];
  copiedId: string | null;
  clearing: boolean;
  onCopy: (text: string, id: string) => void;
  onClear: () => void;
  onGoToLive: () => void;
}) {
  const [confirmClear, setConfirmClear] = useState(false);
  return (
    <section className="view" aria-labelledby="history-heading">
      <div className="view-intro-row">
        <div>
          <div className="eyebrow">Local recordings</div>
          <h2 id="history-heading">History</h2>
          <p className="view-lede">
            Finished transcripts stay on this device until you clear them.
          </p>
        </div>
        {history.length > 0 ? (
          <div className="clear-history-control">
            {!confirmClear ? (
              <button
                className="button button-quiet"
                type="button"
                onClick={() => setConfirmClear(true)}
              >
                <Trash2 size={15} strokeWidth={1.9} aria-hidden="true" /> Clear
                History
              </button>
            ) : (
              <div
                className="confirm-actions"
                role="group"
                aria-label="Confirm clear History"
              >
                <span>Clear all transcripts?</span>
                <button
                  className="button button-danger"
                  type="button"
                  onClick={onClear}
                  disabled={clearing}
                >
                  {clearing ? (
                    <Loader2
                      className="spin"
                      size={15}
                      strokeWidth={2}
                      aria-hidden="true"
                    />
                  ) : (
                    <Check size={15} strokeWidth={2} aria-hidden="true" />
                  )}{" "}
                  Yes, clear
                </button>
                <button
                  className="icon-button"
                  type="button"
                  aria-label="Keep History"
                  onClick={() => setConfirmClear(false)}
                >
                  <X size={16} strokeWidth={2} aria-hidden="true" />
                </button>
              </div>
            )}
          </div>
        ) : null}
      </div>
      {history.length === 0 ? (
        <div className="empty-panel">
          <div className="empty-signal" aria-hidden="true">
            <HistoryIcon size={23} strokeWidth={1.7} />
          </div>
          <h3>No committed transcripts yet.</h3>
          <p>
            When you stop a session, the committed words will appear here for
            quick copy.
          </p>
          <button
            className="button button-primary"
            type="button"
            onClick={onGoToLive}
          >
            <Mic2 size={16} strokeWidth={2} aria-hidden="true" /> Go to Live
          </button>
        </div>
      ) : (
        <ol className="recording-list">
          {history.map((item, index) => (
            <li className="recording-row" key={item.id}>
              <div className="recording-index" aria-hidden="true">
                {String(index + 1).padStart(2, "0")}
              </div>
              <div className="recording-content">
                <p>{item.text}</p>
                <div className="recording-meta">
                  <time dateTime={item.createdAt}>
                    {formatHistoryDate(item.createdAt)}
                  </time>
                  {item.durationSeconds > 0 ? (
                    <>
                      <span aria-hidden="true">·</span>
                      <span>{formatDuration(item.durationSeconds)}</span>
                    </>
                  ) : null}
                  <span aria-hidden="true">·</span>
                  <span>{item.modelName}</span>
                  {item.source === "demo" ? (
                    <span className="source-note">Preview fixture</span>
                  ) : null}
                </div>
              </div>
              <button
                className="button button-quiet copy-history"
                type="button"
                onClick={() => onCopy(item.text, item.id)}
              >
                {copiedId === item.id ? (
                  <Check size={15} strokeWidth={2.2} aria-hidden="true" />
                ) : (
                  <Copy size={15} strokeWidth={2} aria-hidden="true" />
                )}
                {copiedId === item.id ? "Copied" : "Copy"}
              </button>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

function WordsView({
  words,
  onAdd,
  onUpdate,
  onDelete,
}: {
  words: WordEntry[];
  onAdd: (text: string) => Promise<void>;
  onUpdate: (id: string, text: string) => Promise<void>;
  onDelete: (id: string) => Promise<void>;
}) {
  const [newWord, setNewWord] = useState("");
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [error, setError] = useState("");
  const draftFor = (word: WordEntry) => drafts[word.id] ?? word.text;
  const submitNew = async () => {
    setError("");
    setBusyKey("new");
    try {
      await onAdd(newWord);
      setNewWord("");
    } catch (errorValue) {
      setError(errorMessage(errorValue));
    } finally {
      setBusyKey(null);
    }
  };
  const saveExisting = async (word: WordEntry) => {
    setError("");
    setBusyKey(word.id);
    try {
      await onUpdate(word.id, draftFor(word));
      setDrafts((current) => {
        const next = { ...current };
        delete next[word.id];
        return next;
      });
    } catch (errorValue) {
      setError(errorMessage(errorValue));
    } finally {
      setBusyKey(null);
    }
  };
  const remove = async (word: WordEntry) => {
    setError("");
    setBusyKey(word.id);
    try {
      await onDelete(word.id);
    } catch (errorValue) {
      setError(errorMessage(errorValue));
    } finally {
      setBusyKey(null);
    }
  };
  return (
    <section className="view" aria-labelledby="words-heading">
      <div className="view-intro-row">
        <div>
          <div className="eyebrow">Vocabulary</div>
          <h2 id="words-heading">Custom words</h2>
          <p className="view-lede">
            Add names, product terms, and phrases that should stay intact while
            you speak.
          </p>
        </div>
        <div className="word-count">
          <strong>{words.length}</strong>
          <span>{words.length === 1 ? "entry" : "entries"}</span>
        </div>
      </div>
      <form
        className="add-word-form"
        onSubmit={(event) => {
          event.preventDefault();
          void submitNew();
        }}
      >
        <label htmlFor="new-word">Add a word or phrase</label>
        <div className="form-row">
          <input
            id="new-word"
            value={newWord}
            onChange={(event) => setNewWord(event.currentTarget.value)}
            placeholder="e.g. Voicel"
            maxLength={80}
          />
          <button
            className="button button-primary"
            type="submit"
            disabled={!newWord.trim() || busyKey === "new"}
          >
            {busyKey === "new" ? (
              <Loader2
                className="spin"
                size={16}
                strokeWidth={2}
                aria-hidden="true"
              />
            ) : (
              <Plus size={16} strokeWidth={2} aria-hidden="true" />
            )}{" "}
            Add word
          </button>
        </div>
        <span className="field-help">
          One entry can contain spaces. Changes apply to future sessions.
        </span>
      </form>
      {error ? (
        <div className="form-error" role="alert">
          <CircleAlert size={16} strokeWidth={2} aria-hidden="true" />
          {error}
        </div>
      ) : null}
      {words.length === 0 ? (
        <div className="empty-panel compact-empty">
          <div className="empty-signal" aria-hidden="true">
            <BookOpen size={22} strokeWidth={1.7} />
          </div>
          <h3>Your dictionary is empty.</h3>
          <p>Add the words that make your work sound like you.</p>
        </div>
      ) : (
        <ul className="words-list">
          {words.map((word) => {
            const draft = draftFor(word);
            const changed = draft !== word.text;
            return (
              <li className="word-row" key={word.id}>
                <div className="word-symbol" aria-hidden="true">
                  <BookOpen size={17} strokeWidth={1.8} />
                </div>
                <div className="word-field">
                  <label htmlFor={`word-${word.id}`}>Dictionary entry</label>
                  <input
                    id={`word-${word.id}`}
                    value={draft}
                    maxLength={80}
                    onChange={(event) =>
                      setDrafts((current) => ({
                        ...current,
                        [word.id]: event.currentTarget.value,
                      }))
                    }
                  />
                </div>
                <div className="row-actions">
                  <button
                    className="button button-quiet"
                    type="button"
                    disabled={!changed || busyKey === word.id}
                    onClick={() => void saveExisting(word)}
                  >
                    {busyKey === word.id ? (
                      <Loader2
                        className="spin"
                        size={15}
                        strokeWidth={2}
                        aria-hidden="true"
                      />
                    ) : (
                      <Save size={15} strokeWidth={1.9} aria-hidden="true" />
                    )}{" "}
                    Save
                  </button>
                  <button
                    className="icon-button danger-icon"
                    type="button"
                    aria-label={`Delete ${word.text}`}
                    disabled={busyKey === word.id}
                    onClick={() => void remove(word)}
                  >
                    <Trash2 size={16} strokeWidth={1.9} aria-hidden="true" />
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

function ModelsView({
  models,
  selectedId,
  selectedModel,
  operation,
  onInstall,
  onSelect,
}: {
  models: ModelInfo[];
  selectedId: string;
  selectedModel?: ModelInfo;
  operation: ModelOperation | null;
  onInstall: (id: string) => void;
  onSelect: (id: string) => void;
}) {
  return (
    <section className="view" aria-labelledby="models-heading">
      <div className="view-intro-row">
        <div>
          <div className="eyebrow">Local engine</div>
          <h2 id="models-heading">Models</h2>
          <p className="view-lede">
            Keep transcription on this device. Install only the models you plan
            to use.
          </p>
        </div>
        <div className="model-readout-block">
          <span>Selected model</span>
          <strong>
            {selectedModel?.name ?? "None"}
          </strong>
        </div>
      </div>
      <div className="models-note">
        <Cpu size={17} strokeWidth={1.8} aria-hidden="true" />
        <span>
          Downloads can take a moment. Until a model is installed and selected,
          Live stays unavailable.
        </span>
      </div>
      <div
        className="model-list"
        role="list"
        aria-label="Available local models"
      >
        {models.map((model) => {
          const current = operation?.id === model.id ? operation : null;
          const selected = model.id === selectedId;
          const busy = Boolean(current && !current.error);
          const selectDisabled = Boolean(
            operation?.kind === "select" && !current?.error,
          );
          return (
            <article
              className={`model-row${selected ? " is-selected" : ""}`}
              key={model.id}
              role="listitem"
            >
              <div className="model-state-mark" aria-hidden="true">
                {selected ? (
                  <CircleCheck size={20} strokeWidth={1.8} />
                ) : model.installed ? (
                  <Check size={18} strokeWidth={2.2} />
                ) : (
                  <Download size={18} strokeWidth={1.8} />
                )}
              </div>
              <div className="model-details">
                <div className="model-name-line">
                  <h3>{model.name}</h3>
                  {model.recommended ? (
                    <span className="recommended-tag">Recommended</span>
                  ) : null}
                </div>
                <p>{model.description}</p>
                <div className="model-meta">
                  <span>{model.size}</span>
                  <span aria-hidden="true">·</span>
                  <span>
                    {model.streaming === "true"
                      ? "True streaming"
                      : "Incremental preview"}
                  </span>
                  <span aria-hidden="true">·</span>
                  <span>{model.languages}</span>
                  <span aria-hidden="true">·</span>
                  <span>
                    {model.installed ? "Installed locally" : "Not installed"}
                  </span>
                </div>
                {current?.error ? (
                  <div className="model-error" role="alert">
                    <CircleAlert size={14} strokeWidth={2} aria-hidden="true" />
                    {current.error}
                  </div>
                ) : null}
              </div>
              <div className="model-actions">
                {model.installed ? (
                  <>
                    {selected ? (
                      <span className="active-label">Active</span>
                    ) : (
                      <button
                        className="button button-quiet"
                        type="button"
                        disabled={busy || selectDisabled}
                        onClick={() => onSelect(model.id)}
                      >
                        {current?.kind === "select" && current.error
                          ? "Retry select"
                          : "Select"}
                      </button>
                    )}
                  </>
                ) : (
                  <button
                    className="button button-primary"
                    type="button"
                    disabled={busy}
                    onClick={() => onInstall(model.id)}
                  >
                    {current?.kind === "install" ? (
                      <Loader2
                        className="spin"
                        size={15}
                        strokeWidth={2}
                        aria-hidden="true"
                      />
                    ) : (
                      <Download size={15} strokeWidth={2} aria-hidden="true" />
                    )}
                    {current?.error
                      ? "Retry download"
                      : current?.kind === "install"
                        ? "Downloading…"
                        : "Install"}
                  </button>
                )}
                {current?.kind === "select" && !current.error ? (
                  <span className="busy-label">
                    <Loader2
                      className="spin"
                      size={14}
                      strokeWidth={2}
                      aria-hidden="true"
                    />{" "}
                    Selecting…
                  </span>
                ) : null}
              </div>
            </article>
          );
        })}
      </div>
    </section>
  );
}

function shortcutFromEvent(
  event: ReactKeyboardEvent<HTMLButtonElement>,
): string | null {
  const modifiers = [
    event.ctrlKey ? "Ctrl" : "",
    event.altKey ? "Alt" : "",
    event.shiftKey ? "Shift" : "",
    event.metaKey ? "Meta" : "",
  ].filter(Boolean);
  if (["Control", "Shift", "Alt", "Meta"].includes(event.key)) {
    return modifiers.length >= 2 ? modifiers.join("+") : null;
  }
  let key = event.code;
  if (key.startsWith("Key")) key = key.slice(3);
  if (key.startsWith("Digit")) key = key.slice(5);
  return [...modifiers, key].join("+");
}

function ShortcutRecorder({
  id,
  value,
  help,
  onChange,
  onCapture,
}: {
  id: string;
  value: string;
  help: string;
  onChange: (value: string) => void;
  onCapture: (active: boolean) => Promise<void>;
}) {
  const [capturing, setCapturing] = useState(false);
  useEffect(
    () => () => {
      void onCapture(false);
    },
    [onCapture],
  );
  const begin = () => {
    setCapturing(true);
    void onCapture(true);
  };
  const finish = () => {
    setCapturing(false);
    void onCapture(false);
  };
  const record = (event: ReactKeyboardEvent<HTMLButtonElement>) => {
    if (!capturing) return;
    event.preventDefault();
    event.stopPropagation();
    const shortcut = shortcutFromEvent(event);
    if (!shortcut) return;
    onChange(shortcut);
    finish();
  };
  return (
    <>
      <button
        className={`shortcut-recorder${capturing ? " is-capturing" : ""}`}
        id={id}
        type="button"
        aria-pressed={capturing}
        aria-describedby={`${id}-help`}
        data-shortcut-recorder
        onClick={begin}
        onBlur={finish}
        onKeyDown={record}
      >
        <Keyboard size={15} strokeWidth={1.8} aria-hidden="true" />
        <span>{capturing ? "Press your shortcut…" : value}</span>
      </button>
      <small id={`${id}-help`}>
        {capturing ? "Press two modifiers, or a complete key chord." : help}
      </small>
    </>
  );
}

function SettingsView({
  settings,
  saving,
  onSave,
  onShortcutCapture,
}: {
  settings: AppSettings;
  saving: boolean;
  onSave: (settings: AppSettings) => Promise<void>;
  onShortcutCapture: (active: boolean) => Promise<void>;
}) {
  const [draft, setDraft] = useState(settings);
  const [error, setError] = useState("");
  const dirty = JSON.stringify(draft) !== JSON.stringify(settings);
  useEffect(() => setDraft(settings), [settings]);
  const update = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) =>
    setDraft((current) => ({ ...current, [key]: value }));
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError("");
    try {
      await onSave({
        ...draft,
        historyLimit: Math.max(1, Math.min(3, Math.round(draft.historyLimit))),
      });
    } catch (errorValue) {
      setError(errorMessage(errorValue));
    }
  };
  return (
    <section className="view settings-view" aria-labelledby="settings-heading">
      <div className="view-intro-row">
        <div>
          <div className="eyebrow">Workspace behavior</div>
          <h2 id="settings-heading">Settings</h2>
          <p className="view-lede">
            Make Voicel feel native to the way you work in Windows.
          </p>
        </div>
        <div className="settings-save-state" role="status">
          {dirty ? "Unsaved changes" : "All changes saved"}
        </div>
      </div>
      <form onSubmit={(event) => void submit(event)}>
        <div className="settings-section">
          <div className="settings-section-heading">
            <div className="settings-icon" aria-hidden="true">
              <Settings2 size={18} strokeWidth={1.8} />
            </div>
            <div>
              <h3>Startup &amp; presence</h3>
              <p>Choose how the utility behaves when Windows starts.</p>
            </div>
          </div>
          <div className="settings-rows">
            <label className="setting-row">
              <span>
                <strong>Start with Windows</strong>
                <small>Open Voicel when you sign in.</small>
              </span>
              <input
                type="checkbox"
                checked={draft.startWithWindows}
                onChange={(event) =>
                  update("startWithWindows", event.currentTarget.checked)
                }
              />
            </label>
            <label className="setting-row">
              <span>
                <strong>Keep in the system tray</strong>
                <small>
                  Close the window without ending the native listener.
                </small>
              </span>
              <input
                type="checkbox"
                checked={draft.keepInTray}
                onChange={(event) =>
                  update("keepInTray", event.currentTarget.checked)
                }
              />
            </label>
            <label className="setting-row">
              <span>
                <strong>Start hidden</strong>
                <small>Stay in the tray until you open the window.</small>
              </span>
              <input
                type="checkbox"
                checked={draft.startHidden}
                onChange={(event) =>
                  update("startHidden", event.currentTarget.checked)
                }
              />
            </label>
            <div className="setting-row">
              <span>
                <strong>Unload model immediately</strong>
                <small>
                  Model memory is released as soon as dictation finishes.
                </small>
              </span>
              <span className="active-label">Always on</span>
            </div>
          </div>
        </div>
        <div className="settings-section">
          <div className="settings-section-heading">
            <div className="settings-icon" aria-hidden="true">
              <Keyboard size={18} strokeWidth={1.8} />
            </div>
            <div>
              <h3>Hotkeys</h3>
              <p>
                Start and stop with one toggle; cancel separately. Neither
                requires holding a key down.
              </p>
            </div>
          </div>
          <div className="settings-form-grid">
            <div className="field-group">
              <label htmlFor="recording-hotkey">Recording toggle</label>
              <ShortcutRecorder
                id="recording-hotkey"
                value={draft.hotkey}
                onChange={(value) => update("hotkey", value)}
                onCapture={onShortcutCapture}
                help="Default: Ctrl+Shift+Space"
              />
            </div>
            <div className="field-group">
              <label htmlFor="cancel-hotkey">Cancel recording</label>
              <ShortcutRecorder
                id="cancel-hotkey"
                value={draft.cancelHotkey}
                onChange={(value) => update("cancelHotkey", value)}
                onCapture={onShortcutCapture}
                help="Default: Escape"
              />
            </div>
          </div>
        </div>
        <div className="settings-section">
          <div className="settings-section-heading">
            <div className="settings-icon" aria-hidden="true">
              <Clipboard size={18} strokeWidth={1.8} />
            </div>
            <div>
              <h3>Text delivery</h3>
              <p>Decide what happens after a transcript is committed.</p>
            </div>
          </div>
          <div className="settings-form-grid">
            <div className="field-group">
              <label htmlFor="paste-method">Paste method</label>
              <select
                id="paste-method"
                value={draft.pasteMethod}
                onChange={(event) =>
                  update(
                    "pasteMethod",
                    event.currentTarget.value as AppSettings["pasteMethod"],
                  )
                }
              >
                <option value="paste">Paste into the focused app</option>
                <option value="type">Type into the focused app</option>
              </select>
              <small>
                Change this if an app does not accept simulated paste.
              </small>
            </div>
            <div className="field-group">
              <label htmlFor="history-limit">History limit</label>
              <input
                id="history-limit"
                type="number"
                min={1}
                max={3}
                step={1}
                value={draft.historyLimit}
                onChange={(event) =>
                  update("historyLimit", Number(event.currentTarget.value))
                }
              />
              <small>Keep only the latest 1 to 3 transcripts.</small>
            </div>
          </div>
        </div>
        {error ? (
          <div className="form-error" role="alert">
            <CircleAlert size={16} strokeWidth={2} aria-hidden="true" />
            {error}
          </div>
        ) : null}
        <div className="settings-actions">
          <button
            className="button button-primary"
            type="submit"
            disabled={!dirty || saving}
          >
            {saving ? (
              <Loader2
                className="spin"
                size={16}
                strokeWidth={2}
                aria-hidden="true"
              />
            ) : (
              <Save size={16} strokeWidth={2} aria-hidden="true" />
            )}
            {saving ? "Saving…" : "Save changes"}
          </button>
          {dirty ? (
            <button
              className="button button-quiet"
              type="button"
              onClick={() => setDraft(settings)}
            >
              Discard changes
            </button>
          ) : null}
        </div>
      </form>
    </section>
  );
}
