/* @vitest-environment jsdom */
import { act, waitFor } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";
const eventMocks = vi.hoisted(() => {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();
  return {
    listen: vi.fn(
      async (
        event: string,
        next: (event: { payload: unknown }) => void
      ) => {
        handlers.set(event, next);
        return vi.fn();
      }
    ),
    emit(eventOrPayload: unknown, payload?: unknown) {
      if (typeof eventOrPayload === "string" && payload !== undefined) {
        handlers.get(eventOrPayload)?.({ payload });
      } else {
        handlers.get("recording:state-changed")?.({ payload: eventOrPayload });
      }
    },
  };
});

const apiMocks = vi.hoisted(() => ({
  getRecordingState: vi.fn().mockResolvedValue({
    phase: "preparing",
    revision: 1,
  }),
}));

vi.mock("@tauri-apps/api/event", () => ({ listen: eventMocks.listen }));
vi.mock("./lib/api", () => ({
  getRecordingState: apiMocks.getRecordingState,
}));

beforeAll(async () => {
  document.body.innerHTML = '<div id="overlay-root"></div>';
  // overlay.tsx mounts at import time, so the test DOM must exist first.
  await import("./overlay");
});

describe("recording overlay", () => {
  it("shows only Preparing/Recording and ignores stale lifecycle snapshots", async () => {
    await waitFor(() => {
      expect(document.querySelector(".bg-slate-500")).not.toBeNull();
    });

    await waitFor(() => {
      expect(eventMocks.listen).toHaveBeenCalledWith(
        "recording:state-changed",
        expect.any(Function)
      );
    });

    const pill = document.querySelector<HTMLElement>("[data-pill]");
    expect(pill?.className).toContain("rounded-full");
    expect(pill?.className).toContain("bg-black/90");
    expect(pill?.className).not.toMatch(/shadow|ring|border/);

    // Existing shared capture-start behavior remains red.
    act(() => {
      eventMocks.emit({ phase: "recording", revision: 2 });
    });
    await waitFor(() => {
      expect(document.querySelectorAll(".bg-red-500")).toHaveLength(2);
      expect(document.querySelector(".bg-slate-500")).toBeNull();
    });

    // Release/finalization is invisible.
    act(() => {
      eventMocks.emit({ phase: "finalizing", revision: 3 });
    });
    await waitFor(() => {
      expect(document.querySelector("[data-pill]")).toBeNull();
    });

    // A stale delayed Started event cannot re-show/recolor after release.
    act(() => {
      eventMocks.emit({ phase: "recording", revision: 2 });
    });
    await waitFor(() => {
      expect(document.querySelector("[data-pill]")).toBeNull();
    });

    // A newer press reuses the existing grey Preparing visualization.
    act(() => {
      eventMocks.emit({ phase: "preparing", revision: 4 });
    });
    await waitFor(() => {
      expect(document.querySelector(".bg-slate-500")).not.toBeNull();
      expect(document.querySelectorAll(".bg-red-500")).toHaveLength(0);
    });

    // Stale stop/completion from the previous identity cannot hide the new one.
    act(() => {
      eventMocks.emit({ phase: "finalizing", revision: 3 });
    });
    await waitFor(() => {
      expect(document.querySelector(".bg-slate-500")).not.toBeNull();
    });

    // Malformed runtime payloads are ignored rather than changing UI.
    act(() => {
      eventMocks.emit(null);
      eventMocks.emit({ phase: "recording", revision: "newer" });
    });
    await waitFor(() => {
      expect(document.querySelector(".bg-slate-500")).not.toBeNull();
    });

    // Current capture start still changes grey to red.
    act(() => {
      eventMocks.emit({ phase: "recording", revision: 5 });
    });
    await waitFor(() => {
      expect(document.querySelectorAll(".bg-red-500")).toHaveLength(2);
    });

    // Muted state shows the pill with grey MicOff icon.
    act(() => {
      eventMocks.emit({ phase: "muted", revision: 6 });
    });
    await waitFor(() => {
      expect(document.querySelector("[data-pill]")).not.toBeNull();
      expect(document.querySelector("[data-testid='mic-off-icon']")).not.toBeNull();
      expect(document.querySelector(".text-slate-400")).not.toBeNull();
      expect(document.querySelectorAll(".bg-red-500")).toHaveLength(0);
    });

    // Releasing while muted transitions to finalizing and hides the pill.
    act(() => {
      eventMocks.emit({ phase: "finalizing", revision: 7 });
    });
    await waitFor(() => {
      expect(document.querySelector("[data-pill]")).toBeNull();
    });

    // Idle is also invisible.
    act(() => {
      eventMocks.emit({ phase: "idle", revision: 8 });
    });
    await waitFor(() => {
      expect(document.querySelector("[data-pill]")).toBeNull();
    });
  });
  it("shows Processing... and On Clipboard! during clipboard delivery lifecycle", async () => {
    // 1. Enter finalizing with clipboard processing
    act(() => {
      eventMocks.emit("recording:state-changed", { phase: "finalizing", revision: 9 });
      eventMocks.emit("delivery:clipboard-status", { status: "processing" });
    });
    await waitFor(() => {
      const processing = document.querySelector("[data-testid='clipboard-status-processing']");
      expect(processing).not.toBeNull();
      expect(processing?.textContent).toBe("Processing...");
      expect(processing?.className).toContain("text-slate-400");
    });

    // 2. Deliver text to clipboard -> shows "On Clipboard!" in green
    act(() => {
      eventMocks.emit("delivery:clipboard-status", { status: "copied" });
    });
    await waitFor(() => {
      const copied = document.querySelector("[data-testid='clipboard-status-copied']");
      expect(copied).not.toBeNull();
      expect(copied?.textContent).toBe("Copied to Clipboard!");
      expect(copied?.className).toContain("text-accent");
      expect(document.querySelector("[data-testid='clipboard-status-processing']")).toBeNull();
    });

    // 3. New recording begins -> immediately clears copied state and returns to preparing
    act(() => {
      eventMocks.emit("recording:state-changed", { phase: "preparing", revision: 10 });
    });
    await waitFor(() => {
      expect(document.querySelector("[data-testid='clipboard-status-copied']")).toBeNull();
      expect(document.querySelector(".bg-slate-500")).not.toBeNull();
    });
  });
});
