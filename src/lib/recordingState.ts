import type { RecordingState } from "./types";

export const INITIAL_RECORDING_STATE: RecordingState = {
  phase: "idle",
  revision: 0,
};

/** Keep the newest authoritative backend snapshot across event/command races. */
export function reconcileRecordingState(
  current: RecordingState,
  incoming: RecordingState
): RecordingState {
  return incoming.revision >= current.revision ? incoming : current;
}
