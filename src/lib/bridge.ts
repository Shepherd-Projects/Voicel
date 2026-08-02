import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AppSnapshot, Settings, TranscriptRevision } from "../types";

export const isDesktopRuntime = "__TAURI_INTERNALS__" in window;

const preview: AppSnapshot = {
  phase: "ready",
  activeModel: "zipformer-en",
  models: [
    {
      id: "zipformer-en",
      name: "Live English",
      description:
        "Low-latency CPU transcription that decodes continuously as audio arrives.",
      streaming: "true",
      languages: "English",
      sizeLabel: "About 510 MB",
      installed: true,
    },
    {
      id: "parakeet-v3",
      name: "Parakeet v3",
      description:
        "Higher-accuracy multilingual transcription with a continuously revised live preview.",
      streaming: "incremental",
      languages: "25 European languages",
      sizeLabel: "About 640 MB",
      installed: false,
    },
  ],
  history: [
    {
      id: "preview-1",
      text: "The useful thing about a voice tool is that it gets out of the way, but never leaves you wondering whether it heard you.",
      createdAt: new Date(Date.now() - 9 * 60_000).toISOString(),
      modelName: "Live English",
    },
    {
      id: "preview-2",
      text: "Remember to preserve the clipboard after inserting the transcript.",
      createdAt: new Date(Date.now() - 58 * 60_000).toISOString(),
      modelName: "Parakeet v3",
    },
  ],
  customWords: ["Voicel", "Parakeet", "sherpa-onnx"],
  stableText: "",
  revisingText: "",
  elapsedMs: 0,
  inputLevel: 0,
  settings: {
    toggleShortcut: "Ctrl+Shift+Space",
    cancelShortcut: "Escape",
    launchAtStartup: false,
    startHidden: true,
    showTrayIcon: true,
    unloadImmediately: true,
    insertionMethod: "clipboard_receipt",
    historyLimit: 3,
  },
};

let timer: number | undefined;
let revision = 0;
const listeners = new Set<(value: TranscriptRevision) => void>();
const previewWords =
  "This is genuine streaming text arriving while you speak, so a long thought never turns into a long wait at the end.".split(
    " ",
  );

function emit(value: TranscriptRevision) {
  listeners.forEach((listener) => listener(value));
}

const desktopBridge = {
  snapshot: () => invoke<AppSnapshot>("app_snapshot"),
  start: () => invoke<void>("start_recording"),
  stop: () => invoke<void>("stop_recording"),
  cancel: () => invoke<void>("cancel_recording"),
  updateSettings: (patch: Partial<Settings>) =>
    invoke<Settings>("update_settings", { patch }),
  addWord: (word: string) => invoke<void>("add_custom_word", { word }),
  removeWord: (word: string) => invoke<void>("remove_custom_word", { word }),
  selectModel: (modelId: string) => invoke<void>("select_model", { modelId }),
  installModel: (modelId: string) => invoke<void>("install_model", { modelId }),
  copy: (text: string) => invoke<void>("copy_text", { text }),
  onRevision: (listener: (value: TranscriptRevision) => void) =>
    listen<TranscriptRevision>("transcript-revision", (event) =>
      listener(event.payload),
    ),
  minimize: () => getCurrentWindow().minimize(),
  hide: () => getCurrentWindow().hide(),
};

const previewBridge = {
  snapshot: async () => structuredClone(preview),
  start: async () => {
    window.clearInterval(timer);
    preview.phase = "recording";
    preview.stableText = "";
    preview.revisingText = "";
    preview.elapsedMs = 0;
    revision = 0;
    timer = window.setInterval(() => {
      revision += 1;
      const count = Math.min(previewWords.length, Math.ceil(revision / 2));
      const stableCount = Math.max(0, count - 3);
      preview.stableText = previewWords.slice(0, stableCount).join(" ");
      preview.revisingText = previewWords.slice(stableCount, count).join(" ");
      preview.elapsedMs = revision * 420;
      preview.inputLevel = 0.18 + Math.abs(Math.sin(revision * 1.7)) * 0.72;
      emit({
        revision,
        stableText: preview.stableText,
        revisingText: preview.revisingText,
        final: false,
        elapsedMs: preview.elapsedMs,
        inputLevel: preview.inputLevel,
      });
    }, 420);
  },
  stop: async () => {
    window.clearInterval(timer);
    preview.phase = "ready";
    preview.stableText = `${preview.stableText} ${preview.revisingText}`.trim();
    preview.revisingText = "";
    preview.inputLevel = 0;
    emit({
      revision: revision + 1,
      stableText: preview.stableText,
      revisingText: "",
      final: true,
      elapsedMs: preview.elapsedMs,
      inputLevel: 0,
    });
  },
  cancel: async () => {
    window.clearInterval(timer);
    preview.phase = "ready";
    preview.stableText = "";
    preview.revisingText = "";
    preview.elapsedMs = 0;
    preview.inputLevel = 0;
  },
  updateSettings: async (patch: Partial<Settings>) =>
    (preview.settings = { ...preview.settings, ...patch }),
  addWord: async (word: string) => {
    if (
      !preview.customWords.some(
        (item) => item.toLocaleLowerCase() === word.toLocaleLowerCase(),
      )
    )
      preview.customWords.push(word);
  },
  removeWord: async (word: string) => {
    preview.customWords = preview.customWords.filter((item) => item !== word);
  },
  selectModel: async (modelId: string) => {
    preview.activeModel = modelId;
  },
  installModel: async (modelId: string) => {
    const model = preview.models.find((item) => item.id === modelId);
    if (model) model.installed = true;
  },
  copy: async (text: string) => navigator.clipboard.writeText(text),
  onRevision: async (
    listener: (value: TranscriptRevision) => void,
  ): Promise<UnlistenFn> => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
  minimize: async () => undefined,
  hide: async () => undefined,
};

export const bridge = isDesktopRuntime ? desktopBridge : previewBridge;
