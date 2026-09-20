import { describe, expect, it } from "vitest";
import { INITIAL_RECORDING_STATE, reconcileRecordingState } from "./recordingState";

const preparing = { phase: "preparing", revision: 1 } as const;
const recording = { phase: "recording", revision: 2 } as const;
const finalizing = { phase: "finalizing", revision: 3 } as const;

describe("recording state reconciliation", () => {
  it("applies current and newer authoritative snapshots", () => {
    expect(reconcileRecordingState(INITIAL_RECORDING_STATE, preparing)).toBe(preparing);
    expect(reconcileRecordingState(preparing, recording)).toBe(recording);
    expect(reconcileRecordingState(recording, finalizing)).toBe(finalizing);
  });

  it("rejects a stale command response after a newer event", () => {
    expect(reconcileRecordingState(recording, preparing)).toBe(recording);
    expect(reconcileRecordingState(finalizing, recording)).toBe(finalizing);
  });
});
