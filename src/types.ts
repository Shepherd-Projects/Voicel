export type AppView = "live" | "history" | "words" | "models" | "settings";
export type AppPhase = "loading" | "ready" | "recording" | "finalizing" | "error";

export interface Settings {
  toggleShortcut: string;
  cancelShortcut: string;
  launchAtStartup: boolean;
  startHidden: boolean;
  showTrayIcon: boolean;
  unloadImmediately: boolean;
  insertionMethod: "clipboard_receipt" | "typing";
  historyLimit: 1 | 2 | 3;
}

export interface ModelInfo {
  id: string;
  name: string;
  description: string;
  streaming: "true" | "incremental";
  languages: string;
  sizeLabel: string;
  installed: boolean;
  progress?: number;
}

export interface HistoryEntry {
  id: string;
  text: string;
  createdAt: string;
  modelName: string;
}

export interface AppSnapshot {
  phase: AppPhase;
  activeModel: string;
  models: ModelInfo[];
  history: HistoryEntry[];
  customWords: string[];
  stableText: string;
  revisingText: string;
  elapsedMs: number;
  inputLevel: number;
  settings: Settings;
  error?: string;
}

export interface TranscriptRevision {
  revision: number;
  stableText: string;
  revisingText: string;
  final: boolean;
  elapsedMs: number;
  inputLevel: number;
}
