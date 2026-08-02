import { AudioLines, X } from "lucide-react";
import { useEffect, useState } from "react";
import { bridge } from "./lib/bridge";
import type { AppPhase, TranscriptRevision } from "./types";
import "./overlay.css";

const initial: TranscriptRevision = {
  revision: 0,
  stableText: "",
  revisingText: "",
  final: false,
  elapsedMs: 0,
  inputLevel: 0,
};

export default function Overlay() {
  const [revision, setRevision] = useState(initial);
  const [phase, setPhase] = useState<AppPhase>("loading");

  useEffect(() => {
    const unlistenRevision = bridge.onRevision(setRevision);
    const unlistenPhase = bridge.onSessionPhase(({ phase: nextPhase }) => {
      setPhase(nextPhase);
      if (nextPhase === "loading") setRevision(initial);
    });
    return () => {
      void unlistenRevision.then((dispose) => dispose());
      void unlistenPhase.then((dispose) => dispose());
    };
  }, []);

  const isLoading = phase === "loading";
  const transcript = `${revision.stableText} ${revision.revisingText}`.trim();
  const tail = transcript.split(/\s+/).slice(-12).join(" ");

  return (
    <main
      className="recording-overlay"
      data-phase={isLoading ? "loading" : "recording"}
      aria-busy={isLoading}
    >
      <span className="sr-only" role="status" aria-live="polite">
        {isLoading
          ? "Hotkey detected. Loading the speech model. Escape cancels."
          : "Recording started. Use the toggle shortcut to finish or Escape to cancel."}
      </span>
      <div className="overlay-signal" aria-hidden="true">
        <AudioLines size={18} />
        {!isLoading && (
          <i
            style={{
              transform: `scaleY(${Math.max(0.2, revision.inputLevel)})`,
            }}
          />
        )}
      </div>
      <div className="overlay-copy">
        <span>
          <b>{isLoading ? "PREPARING" : "LISTENING"}</b>
          {isLoading ? "MODEL LOADING" : formatElapsed(revision.elapsedMs)}
        </span>
        <p>
          {isLoading
            ? "Preparing speech recognition…"
            : tail || "Speak when you’re ready…"}
        </p>
      </div>
      <div className="overlay-shortcuts">
        <span>{isLoading ? "Hotkey detected" : "Toggle to finish"}</span>
        <span>
          <X size={12} /> Esc cancels
        </span>
      </div>
    </main>
  );
}

function formatElapsed(milliseconds: number) {
  const total = Math.floor(milliseconds / 1000);
  return `${String(Math.floor(total / 60)).padStart(2, "0")}:${String(total % 60).padStart(2, "0")}`;
}
