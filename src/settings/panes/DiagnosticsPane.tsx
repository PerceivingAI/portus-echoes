import React, { useEffect, useRef, useState } from "react";
import { RefreshCw } from "lucide-react";
import { PaneHeader } from "../../components/PaneHeader";
import type {
  DiagnosticsStorageData,
  ProviderValidation,
  TranscriptionFunction,
  TranscriptionType,
} from "../../lib/types";
import { DiagnosticGroupHeading } from "../components/DiagnosticGroupHeading";
import { DiagnosticItemRow } from "../components/DiagnosticItemRow";

export interface DiagnosticsPaneProps {
  diagnostics: DiagnosticsStorageData;
  runningDiagnostics: boolean;
  completedRunSerial: number;
  failedRunSerial: number;
  onRefresh: () => void;
}

const providerText = (value: ProviderValidation | undefined) =>
  value === "validated"
    ? "Validated"
    : value === "none_not_valid"
      ? "None/Not Valid"
      : "-";

const functionText = (value: TranscriptionFunction | undefined) => {
  switch (value) {
    case "send_input":
      return "Native";
    case "xdotool":
      return "X11 Assist";
    case "system":
      return "System";
    case "none_not_valid":
      return "None/Not Valid";
    default:
      return "-";
  }
};

const typeText = (value: TranscriptionType | undefined) => {
  switch (value) {
    case "cursor":
      return "Cursor";
    case "clipboard":
      return "Clipboard";
    case "none_not_valid":
      return "None/Not Valid";
    default:
      return "-";
  }
};

export function DiagnosticsPane({
  diagnostics,
  runningDiagnostics,
  completedRunSerial,
  failedRunSerial,
  onRefresh,
}: DiagnosticsPaneProps) {
  const [statusState, setStatusState] = useState<
    "running" | "ready" | "unable" | "settled"
  >("settled");
  const completedRunSerialRef = useRef(completedRunSerial);
  const failedRunSerialRef = useRef(failedRunSerial);

  useEffect(() => {
    if (runningDiagnostics) {
      setStatusState("running");
    } else {
      setStatusState((current) => (current === "running" ? "settled" : current));
    }
  }, [runningDiagnostics]);

  useEffect(() => {
    if (completedRunSerial === completedRunSerialRef.current) return;
    completedRunSerialRef.current = completedRunSerial;
    setStatusState("ready");
  }, [completedRunSerial]);

  useEffect(() => {
    if (failedRunSerial === failedRunSerialRef.current) return;
    failedRunSerialRef.current = failedRunSerial;
    setStatusState("unable");
  }, [failedRunSerial]);

  useEffect(() => {
    if (statusState !== "ready" && statusState !== "unable") return;
    const timer = setTimeout(() => {
      setStatusState("settled");
    }, 1500);
    return () => clearTimeout(timer);
  }, [statusState]);

  const hideSnapshot = runningDiagnostics || statusState === "unable";
  const snapshot = hideSnapshot ? undefined : diagnostics.snapshot;
  const local = snapshot?.local;
  const cloud = snapshot?.cloud;
  const microphone = snapshot?.microphone;
  const access = snapshot?.transcription_access;

  const localPath = providerText(local?.model_path);
  const localFile = providerText(local?.model_file);
  const openaiKey = providerText(cloud?.openai_api_key);
  const openaiModel = providerText(cloud?.openai_model);
  const groqKey = providerText(cloud?.groq_api_key);
  const groqModel = providerText(cloud?.groq_model);
  const micName = microphone?.name ?? "-";
  const micSpecs = microphone?.specs ?? "-";
  const injFunc = functionText(access?.function);
  const injType = typeText(access?.access_type);

  let statusText = "";
  if (statusState === "running") {
    statusText = "Running...";
  } else if (statusState === "ready") {
    statusText = "Ready";
  } else if (statusState === "unable") {
    statusText = "Unable to proceed";
  } else {
    statusText = `Last scan: ${diagnostics.last_scan_display || "-"}`;
  }

  return (
    <div id="pane-diagnostics" className="space-y-4">
      <PaneHeader
        title="Health & Status"
        description="Checks for audio input, models, providers, and delivery method."
        action={
          <button
            type="button"
            onClick={onRefresh}
            disabled={runningDiagnostics}
            onMouseDown={(e) => e.stopPropagation()}
            style={{ WebkitAppRegion: "no-drag" } as React.CSSProperties}
            title="Run Diagnostics"
            aria-label="Run Diagnostics"
            className="flex h-7 w-7 items-center justify-center rounded-md text-slate-400 hover:text-slate-200 transition-colors focus:outline-none cursor-pointer shrink-0 disabled:opacity-40 -translate-x-[5px]"
          >
            <RefreshCw
              className={`w-4 h-4 ${runningDiagnostics ? "animate-spin text-accent" : ""}`}
            />
          </button>
        }
      />

      <div className="grid grid-cols-1 sm:grid-cols-2 gap-x-8 gap-y-4">
        <div className="space-y-1 sm:col-start-1 sm:row-start-1">
          <DiagnosticGroupHeading title="Local Provider" status={local?.passed} />
          <DiagnosticItemRow message={`Model Path: ${localPath}`} />
          <DiagnosticItemRow message={`Model File: ${localFile}`} />
        </div>

        <div
          className="space-y-1 sm:col-start-1 sm:row-start-2"
          data-diagnostic-cloud-group
        >
          <DiagnosticGroupHeading title="Cloud Provider" status={cloud?.passed} />
          <DiagnosticItemRow message={`OpenAI API Key: ${openaiKey}`} />
          <DiagnosticItemRow message={`OpenAI Model: ${openaiModel}`} />
          <div className="h-2" aria-hidden="true" data-diagnostic-provider-gap />
          <DiagnosticItemRow message={`Groq API Key: ${groqKey}`} />
          <DiagnosticItemRow message={`Groq Model: ${groqModel}`} />
        </div>

        <div className="space-y-1 sm:col-start-2 sm:row-start-1">
          <DiagnosticGroupHeading title="Microphone" status={microphone?.passed} />
          <DiagnosticItemRow message={`Name: ${micName}`} />
          <DiagnosticItemRow message={`Specs: ${micSpecs}`} />
        </div>

        <div className="space-y-1 sm:col-start-2 sm:row-start-2" data-diagnostic-access-group>
          <DiagnosticGroupHeading title="Transcription Access" status={access?.passed} />
          <DiagnosticItemRow message={`Function: ${injFunc}`} />
          <DiagnosticItemRow message={`Type: ${injType}`} />
          <div className="hidden sm:block h-2" aria-hidden="true" />
          <div className="hidden sm:block py-0.5 min-w-0" aria-hidden="true">
            <p className="text-xs whitespace-nowrap invisible">placeholder</p>
          </div>
          <div
            className="mt-2 py-0.5 text-right text-xs text-slate-400 whitespace-nowrap select-none pointer-events-none sm:mt-0"
            data-diagnostic-status
          >
            {statusText}
          </div>
        </div>
      </div>
    </div>
  );
}
