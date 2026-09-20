import { useState } from "react";
import { errorMessageForCode } from "../../lib/errors";
import { isRecordingState, type UserErrorCode, type UserErrorPayload } from "../../lib/types";
import { useTauriEvent } from "../../hooks/useTauriEvent";

function codeFromPayload(payload: unknown): unknown {
  return typeof payload === "object" && payload !== null
    ? (payload as Partial<UserErrorPayload>).code
    : undefined;
}

export interface StatusMessage {
  text: string;
  variant: "processing" | "copied";
}

export function useApplicationErrors(initialErrorCode?: UserErrorCode) {
  const [error, setError] = useState<string | null>(() =>
    initialErrorCode ? errorMessageForCode(initialErrorCode) : null
  );
  const [statusMessage, setStatusMessage] = useState<StatusMessage | null>(null);

  useTauriEvent<unknown>("transcription:error", (payload) => {
    setStatusMessage(null);
    setError(errorMessageForCode(codeFromPayload(payload)));
  });

  useTauriEvent<unknown>("app:error", (payload) => {
    setStatusMessage(null);
    setError(errorMessageForCode(codeFromPayload(payload)));
  });

  useTauriEvent<unknown>("model:preload:error", (payload) => {
    setStatusMessage(null);
    setError(errorMessageForCode(codeFromPayload(payload)));
  });

  useTauriEvent<unknown>("delivery:clipboard-status", (payload) => {
    if (typeof payload === "object" && payload !== null) {
      const status = (payload as Record<string, unknown>).status;
      if (status === "processing") {
        setError(null);
        setStatusMessage({ text: "Processing...", variant: "processing" });
      } else if (status === "copied") {
        setError(null);
        setStatusMessage({ text: "On Clipboard!", variant: "copied" });
      } else if (status === "idle") {
        setStatusMessage(null);
      }
    }
  });

  useTauriEvent<unknown>("recording:state-changed", (payload) => {
    if (
      isRecordingState(payload) &&
      (payload.phase === "preparing" || payload.phase === "recording")
    ) {
      setError(null);
      setStatusMessage(null);
    }
  });
  useTauriEvent<unknown>("settings:active-provider-changed", () => {
    setStatusMessage(null);
  });

  return { error, setError, statusMessage, setStatusMessage };
}
