import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type RuntimeMode = "tauri" | "demo";
export type HealthState = "ready" | "demo" | "unavailable";
export type SessionEventStatus = "recording" | "stopping" | "error";

export interface AppSettings {
  startWithWindows: boolean;
  keepInTray: boolean;
  startHidden: boolean;
  pasteMethod: "paste" | "type";
  hotkey: string;
  cancelHotkey: string;
  historyLimit: number;
}

export interface WordEntry {
  id: string;
  text: string;
  addedAt: string;
}
export interface HistoryItem {
  id: string;
  text: string;
  createdAt: string;
  durationSeconds: number;
  modelName: string;
  source: RuntimeMode;
}
export interface ModelInfo {
  id: string;
  name: string;
  description: string;
  size: string;
  installed: boolean;
  recommended?: boolean;
  streaming?: "true" | "incremental";
  languages?: string;
}
export interface AppHealth {
  microphone: HealthState;
  backend: HealthState;
  selectedModelId: string;
}
export interface AppSnapshot {
  history: HistoryItem[];
  words: WordEntry[];
  models: ModelInfo[];
  settings: AppSettings;
  health: AppHealth;
}
export interface SessionEvent {
  status: SessionEventStatus;
  committedText?: string;
  liveText?: string;
  level?: number;
  elapsedSeconds?: number;
  error?: string;
}

export interface VoicelAdapter {
  readonly mode: RuntimeMode;
  loadSnapshot(): Promise<AppSnapshot>;
  startRecording(): Promise<void>;
  stopRecording(): Promise<SessionEvent | null>;
  cancelRecording(): Promise<void>;
  subscribeToSession(
    onEvent: (event: SessionEvent) => void,
  ): Promise<UnlistenFn>;
  copyText(text: string): Promise<void>;
  addHistory(item: HistoryItem): Promise<void>;
  clearHistory(): Promise<void>;
  addWord(text: string): Promise<WordEntry>;
  updateWord(id: string, text: string): Promise<WordEntry>;
  deleteWord(id: string): Promise<void>;
  installModel(id: string): Promise<ModelInfo>;
  unloadModel(id: string): Promise<void>;
  selectModel(id: string): Promise<void>;
  saveSettings(settings: AppSettings): Promise<AppSettings>;
  setShortcutCapture(active: boolean): Promise<void>;
}

interface NativeSettings {
  toggleShortcut: string;
  cancelShortcut: string;
  launchAtStartup: boolean;
  startHidden: boolean;
  showTrayIcon: boolean;
  unloadImmediately: boolean;
  insertionMethod: "clipboard_receipt" | "typing" | "literal_typing";
  historyLimit: number;
}
interface NativeModel {
  id: string;
  name: string;
  description: string;
  streaming: "true" | "incremental";
  languages: string;
  sizeLabel: string;
  installed: boolean;
}
interface NativeHistory {
  id: string;
  text: string;
  createdAt: string;
  modelName: string;
}
interface NativeSnapshot {
  phase: "loading" | "ready" | "recording" | "finalizing" | "error";
  activeModel: string;
  models: NativeModel[];
  history: NativeHistory[];
  customWords: string[];
  stableText: string;
  revisingText: string;
  elapsedMs: number;
  inputLevel: number;
  error?: string;
  settings: NativeSettings;
}
interface NativeRevision {
  stableText: string;
  revisingText: string;
  isFinal?: boolean;
  final?: boolean;
  elapsedMs: number;
  inputLevel: number;
  error?: string;
}

const DEFAULT_SETTINGS: AppSettings = {
  startWithWindows: false,
  keepInTray: true,
  startHidden: true,
  pasteMethod: "paste",
  hotkey: "Ctrl+Shift+Space",
  cancelHotkey: "Escape",
  historyLimit: 3,
};
const DEFAULT_MODELS: ModelInfo[] = [
  {
    id: "zipformer-en",
    name: "Live English",
    description:
      "True streaming transcription on CPU. Words appear while you speak.",
    size: "About 510 MB",
    installed: true,
    recommended: true,
    streaming: "true",
    languages: "English",
  },
  {
    id: "parakeet-v3",
    name: "Parakeet v3",
    description:
      "Accurate multilingual transcription with an incrementally revised preview.",
    size: "About 640 MB",
    installed: false,
    streaming: "incremental",
    languages: "25 European languages",
  },
];
const demoState: AppSnapshot = {
  history: [],
  words: [],
  models: DEFAULT_MODELS.map((model) => ({ ...model })),
  settings: { ...DEFAULT_SETTINGS },
  health: {
    microphone: "demo",
    backend: "demo",
    selectedModelId: "zipformer-en",
  },
};
const DEMO_PHRASES = [
  "The live transcript appears",
  "while you are still speaking",
  "and settles as the audio arrives",
];
const DEMO_LEVELS = [0.28, 0.55, 0.4, 0.7, 0.46, 0.62];

type TauriWindow = Window & {
  __TAURI__?: unknown;
  __TAURI_INTERNALS__?: unknown;
};
function isTauriRuntime(): boolean {
  if (typeof window === "undefined") return false;
  const target = window as TauriWindow;
  return Boolean(target.__TAURI__ || target.__TAURI_INTERNALS__);
}
function cloneSnapshot(value: AppSnapshot): AppSnapshot {
  return {
    ...value,
    history: value.history.map((item) => ({ ...item })),
    words: value.words.map((word) => ({ ...word })),
    models: value.models.map((model) => ({ ...model })),
    settings: { ...value.settings },
    health: { ...value.health },
  };
}
function cleanWord(value: string): string {
  return value.trim().replace(/\s+/g, " ");
}
function toSessionEvent(value: NativeRevision): SessionEvent {
  return {
    status: value.error
      ? "error"
      : value.isFinal || value.final
        ? "stopping"
        : "recording",
    committedText: value.stableText,
    liveText: value.revisingText,
    level: value.inputLevel,
    elapsedSeconds: Math.round(value.elapsedMs / 1000),
    error: value.error,
  };
}
function mapSettings(value: NativeSettings): AppSettings {
  return {
    startWithWindows: value.launchAtStartup,
    keepInTray: value.showTrayIcon,
    startHidden: value.startHidden,
    pasteMethod:
      value.insertionMethod === "typing" ||
      value.insertionMethod === "literal_typing"
        ? "type"
        : "paste",
    hotkey: value.toggleShortcut,
    cancelHotkey: value.cancelShortcut,
    historyLimit: value.historyLimit,
  };
}
function mapSnapshot(value: NativeSnapshot): AppSnapshot {
  return {
    history: value.history.map((item) => ({
      ...item,
      durationSeconds: 0,
      source: "tauri",
    })),
    words: value.customWords.map((text) => ({
      id: text,
      text,
      addedAt: new Date(0).toISOString(),
    })),
    models: value.models.map((model) => ({
      id: model.id,
      name: model.name,
      description: model.description,
      size: model.sizeLabel,
      installed: model.installed,
      recommended: model.id === "zipformer-en",
      streaming: model.streaming,
      languages: model.languages,
    })),
    settings: mapSettings(value.settings),
    health: {
      microphone: "ready",
      backend: "ready",
      selectedModelId: value.activeModel,
    },
  };
}

async function waitForNativeSession(): Promise<SessionEvent | null> {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    const snapshot = await invoke<NativeSnapshot>("app_snapshot");
    if (snapshot.phase === "ready" || snapshot.phase === "error") {
      return toSessionEvent({
        stableText: snapshot.stableText,
        revisingText: snapshot.revisingText,
        final: true,
        elapsedMs: snapshot.elapsedMs,
        inputLevel: snapshot.inputLevel,
        error: snapshot.error,
      });
    }
    await new Promise((resolve) => window.setTimeout(resolve, 40));
  }
  throw new Error("Voicel did not finish the recording within 60 seconds.");
}

export function createVoicelAdapter(): VoicelAdapter {
  const mode: RuntimeMode = isTauriRuntime() ? "tauri" : "demo";
  let demoTimer: number | null = null;
  let demoTick = 0;
  const listeners = new Set<(event: SessionEvent) => void>();
  const emitDemo = () => {
    const phraseIndex = Math.min(
      Math.floor(demoTick / 3),
      DEMO_PHRASES.length - 1,
    );
    const committed = DEMO_PHRASES.slice(0, phraseIndex).join(" ");
    const live = DEMO_PHRASES[phraseIndex]
      .split(" ")
      .slice(0, ((demoTick % 3) + 1) * 2)
      .join(" ");
    const event: SessionEvent = {
      status: "recording",
      committedText: committed,
      liveText: live,
      level: DEMO_LEVELS[demoTick % DEMO_LEVELS.length],
      elapsedSeconds: demoTick + 1,
    };
    listeners.forEach((listener) => listener(event));
    demoTick += 1;
  };
  const findModel = (id: string) =>
    demoState.models.find((model) => model.id === id);

  return {
    mode,
    async loadSnapshot() {
      return mode === "tauri"
        ? mapSnapshot(await invoke<NativeSnapshot>("app_snapshot"))
        : cloneSnapshot(demoState);
    },
    async startRecording() {
      if (mode === "tauri") {
        await invoke("start_recording");
        return;
      }
      demoTick = 0;
      emitDemo();
      demoTimer = window.setInterval(emitDemo, 800);
    },
    async stopRecording() {
      if (mode === "tauri") {
        await invoke("stop_recording");
        return waitForNativeSession();
      }
      if (demoTimer !== null) window.clearInterval(demoTimer);
      demoTimer = null;
      return {
        status: "stopping",
        committedText: DEMO_PHRASES.join(" "),
        liveText: "",
        level: 0,
        elapsedSeconds: Math.max(1, demoTick),
      };
    },
    async cancelRecording() {
      if (mode === "tauri") {
        await invoke("cancel_recording");
        await waitForNativeSession();
      } else if (demoTimer !== null) window.clearInterval(demoTimer);
      demoTimer = null;
    },
    async subscribeToSession(onEvent) {
      if (mode === "tauri")
        return listen<NativeRevision>("transcript-revision", (event) =>
          onEvent(toSessionEvent(event.payload)),
        );
      listeners.add(onEvent);
      return () => listeners.delete(onEvent);
    },
    async copyText(text) {
      if (mode === "tauri") await invoke("copy_text", { text });
      else await navigator.clipboard.writeText(text);
    },
    async addHistory() {
      /* Native sessions persist their own final transcript. */
    },
    async clearHistory() {
      if (mode === "tauri") await invoke("clear_history");
      else demoState.history = [];
    },
    async addWord(text) {
      const word = cleanWord(text);
      if (mode === "tauri") await invoke("add_custom_word", { word });
      else
        demoState.words.unshift({
          id: word,
          text: word,
          addedAt: new Date().toISOString(),
        });
      return { id: word, text: word, addedAt: new Date().toISOString() };
    },
    async updateWord(id, text) {
      const word = cleanWord(text);
      if (mode === "tauri") {
        await invoke("remove_custom_word", { word: id });
        await invoke("add_custom_word", { word });
      } else {
        const current = demoState.words.find((entry) => entry.id === id);
        if (current) Object.assign(current, { id: word, text: word });
      }
      return { id: word, text: word, addedAt: new Date().toISOString() };
    },
    async deleteWord(id) {
      if (mode === "tauri") await invoke("remove_custom_word", { word: id });
      else demoState.words = demoState.words.filter((entry) => entry.id !== id);
    },
    async installModel(id) {
      if (mode === "tauri") {
        await invoke("install_model", { modelId: id });
        const updated = mapSnapshot(
          await invoke<NativeSnapshot>("app_snapshot"),
        );
        const model = updated.models.find((candidate) => candidate.id === id);
        if (!model)
          throw new Error("Installed model is missing from the catalog.");
        return model;
      }
      const model = findModel(id);
      if (!model) throw new Error("Unknown model.");
      model.installed = true;
      return { ...model };
    },
    async unloadModel() {
      /* Models are unloaded from memory by the native session policy, not removed from disk. */
    },
    async selectModel(id) {
      if (mode === "tauri") await invoke("select_model", { modelId: id });
      else demoState.health.selectedModelId = id;
    },
    async saveSettings(settings) {
      const normalized = {
        ...settings,
        historyLimit: Math.max(
          1,
          Math.min(3, Math.round(settings.historyLimit)),
        ),
      };
      if (mode === "tauri") {
        const result = await invoke<NativeSettings>("update_settings", {
          patch: {
            toggleShortcut: normalized.hotkey,
            cancelShortcut: normalized.cancelHotkey,
            launchAtStartup: normalized.startWithWindows,
            startHidden: normalized.startHidden,
            showTrayIcon: normalized.keepInTray,
            unloadImmediately: true,
            insertionMethod:
              normalized.pasteMethod === "type"
                ? "typing"
                : "clipboard_receipt",
            historyLimit: normalized.historyLimit,
          },
        });
        return mapSettings(result);
      }
      demoState.settings = { ...normalized };
      return { ...normalized };
    },
    async setShortcutCapture(active) {
      if (mode === "tauri") await invoke("set_shortcut_capture", { active });
    },
  };
}
