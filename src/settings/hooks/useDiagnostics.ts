import { useCallback, useEffect, useRef, useState } from "react";
import { getDiagnosticsState, runDiagnostics } from "../../lib/api";
import type { DiagnosticsStorageData } from "../../lib/types";

const EMPTY_DIAGNOSTICS: DiagnosticsStorageData = {
  last_scan_display: "",
  snapshot: null,
};

export function useDiagnostics() {
  const [diagnostics, setDiagnostics] = useState<DiagnosticsStorageData>(
    EMPTY_DIAGNOSTICS
  );
  const [runningDiagnostics, setRunningDiagnostics] = useState(false);
  const [completedDiagnosticsRuns, setCompletedDiagnosticsRuns] = useState(0);
  const [failedDiagnosticsRuns, setFailedDiagnosticsRuns] = useState(0);
  const completedExplicitRunRef = useRef(false);

  useEffect(() => {
    getDiagnosticsState()
      .then((data) => {
        if (!completedExplicitRunRef.current) {
          setDiagnostics(data);
        }
      })
      .catch(() => {});
  }, []);

  const refreshDiagnostics = useCallback(() => {
    setRunningDiagnostics(true);
    runDiagnostics()
      .then((data) => {
        completedExplicitRunRef.current = true;
        setDiagnostics(data);
        setCompletedDiagnosticsRuns((current) => current + 1);
      })
      .catch(() => {
        setFailedDiagnosticsRuns((current) => current + 1);
      })
      .finally(() => setRunningDiagnostics(false));
  }, []);

  return {
    diagnostics,
    runningDiagnostics,
    completedDiagnosticsRuns,
    failedDiagnosticsRuns,
    refreshDiagnostics,
  };
}
