import React, { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { MicOff } from "lucide-react";
import { createRoot } from "react-dom/client";
import { getRecordingState } from "./lib/api";
import { isRecordingState, type RecordingState } from "./lib/types";
import {
  INITIAL_RECORDING_STATE,
  reconcileRecordingState,
} from "./lib/recordingState";
import "./styles/globals.css";

/**
 * Recording indicator pill (docs/OVERLAY.md): ~180×48 semi-transparent
 * dark rounded shape with a 3-step indicator during recording, plus
 * clipboard delivery feedback:
 * 1. Static grey dot while initializing mic (preparing state)
 * 2. Pulsing red dot when mic stream is hot and live (recording state)
 * 3. Static grey MicOff icon when mic is missing or muted (muted state)
 * 4. Text "Processing..." in grey during finalization
 * 5. Text "On Clipboard!" in green for 2 seconds upon copy completion
 */
interface ClipboardStatusPayload {
  status: "processing" | "copied" | "idle";
}

function isClipboardStatusPayload(value: unknown): value is ClipboardStatusPayload {
  if (typeof value !== "object" || value === null) return false;
  const status = (value as Record<string, unknown>).status;
  return status === "processing" || status === "copied" || status === "idle";
}

export function Overlay() {
  const [state, setState] = useState<RecordingState>(INITIAL_RECORDING_STATE);
  const [clipboardStatus, setClipboardStatus] = useState<"processing" | "copied" | "idle">("idle");

  useEffect(() => {
    let disposed = false;
    let unlistenRecording: (() => void) | undefined;
    let unlistenClipboard: (() => void) | undefined;

    const applySnapshot = (snapshot: RecordingState) => {
      if (disposed) {
        return;
      }
      setState((current) => reconcileRecordingState(current, snapshot));
      if (snapshot.phase === "preparing" || snapshot.phase === "recording") {
        setClipboardStatus("idle");
      }
    };

    void (async () => {
      try {
        const stopListeningRecording = await listen<unknown>(
          "recording:state-changed",
          (event) => {
            if (isRecordingState(event.payload)) applySnapshot(event.payload);
          }
        );
        if (disposed) {
          stopListeningRecording();
          return;
        }
        unlistenRecording = stopListeningRecording;
      } catch {
        return;
      }

      try {
        const stopListeningClipboard = await listen<unknown>(
          "delivery:clipboard-status",
          (event) => {
            if (isClipboardStatusPayload(event.payload)) {
              setClipboardStatus(event.payload.status);
            }
          }
        );
        if (disposed) {
          stopListeningClipboard();
          return;
        }
        unlistenClipboard = stopListeningClipboard;
      } catch {
        return;
      }

      try {
        const snapshot = await getRecordingState();
        if (isRecordingState(snapshot)) applySnapshot(snapshot);
      } catch {
        // Live events remain authoritative if the initial snapshot fails.
      }
    })();

    return () => {
      disposed = true;
      unlistenRecording?.();
      unlistenClipboard?.();
    };
  }, []);

  useEffect(() => {
    if (clipboardStatus === "copied") {
      const timer = setTimeout(() => {
        setClipboardStatus("idle");
      }, 1700);
      return () => clearTimeout(timer);
    }
  }, [clipboardStatus]);

  const isRecordingInteraction =
    state.phase === "preparing" ||
    state.phase === "recording" ||
    state.phase === "muted";
  const isClipboardNotice =
    (state.phase === "finalizing" && clipboardStatus === "processing") ||
    clipboardStatus === "copied";
  const isVisible = isRecordingInteraction || isClipboardNotice;
  const isRecording = state.phase === "recording";

  if (!isVisible) {
    return null;
  }

  return (
    <div className="flex h-screen w-screen items-center justify-center">
      <div
        data-pill
        className="flex h-12 w-[180px] items-center justify-center rounded-full bg-black/90 px-4"
      >
        {clipboardStatus === "copied" ? (
          <span
            data-testid="clipboard-status-copied"
            className="text-sm font-semibold text-accent select-none"
          >
            On Clipboard!
          </span>
        ) : state.phase === "finalizing" || clipboardStatus === "processing" ? (
          <span
            data-testid="clipboard-status-processing"
            className="text-sm font-medium text-slate-400 select-none animate-pulse"
          >
            Processing...
          </span>
        ) : (
          <span className="relative flex h-5 w-5 items-center justify-center">
            {state.phase === "muted" ? (
              <MicOff className="h-5 w-5 text-slate-400" data-testid="mic-off-icon" />
            ) : isRecording ? (
              <>
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-red-500 opacity-75" />
                <span className="relative inline-flex h-3.5 w-3.5 rounded-full bg-red-500 transition-colors duration-200" />
              </>
            ) : (
              <span className="h-3 w-3 rounded-full bg-slate-500 transition-colors duration-200" />
            )}
          </span>
        )}
      </div>
    </div>
  );
}

createRoot(document.getElementById("overlay-root")!).render(
  <React.StrictMode>
    <Overlay />
  </React.StrictMode>
);
