import { AudioLines, X } from "lucide-react";
import { useEffect, useState } from "react";
import { bridge } from "./lib/bridge";
import type { TranscriptRevision } from "./types";
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

  useEffect(() => {
    const unlisten = bridge.onRevision(setRevision);
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  const transcript = `${revision.stableText} ${revision.revisingText}`.trim();
  const tail = transcript.split(/\s+/).slice(-12).join(" ");

  return (
    <main className="recording-overlay">
      <span className="sr-only" role="status" aria-live="polite">
        Recording started. Use the toggle shortcut to finish or Escape to
        cancel.
      </span>
      <div className="overlay-signal" aria-hidden="true">
        <AudioLines size={18} />
        <i
          style={{ transform: `scaleY(${Math.max(0.2, revision.inputLevel)})` }}
        />
      </div>
      <div className="overlay-copy">
        <span>
          <b>LISTENING</b>
          {formatElapsed(revision.elapsedMs)}
        </span>
        <p>{tail || "Speak when you’re ready…"}</p>
      </div>
      <div className="overlay-shortcuts">
        <span>Toggle to finish</span>
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
