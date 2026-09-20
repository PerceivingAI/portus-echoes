/* @vitest-environment jsdom */
import { render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useTauriEvent } from "./useTauriEvent";

const eventMocks = vi.hoisted(() => {
  let callback: ((event: { payload: unknown }) => void) | null = null;
  const unlisten = vi.fn();
  return {
    listen: vi.fn(
      (_event: string, next: (event: { payload: unknown }) => void) => {
        callback = next;
        return Promise.resolve(unlisten);
      }
    ),
    emit(payload: unknown) {
      callback?.({ payload });
    },
    reset() {
      callback = null;
      unlisten.mockClear();
    },
    unlisten,
  };
});

vi.mock("@tauri-apps/api/event", () => ({ listen: eventMocks.listen }));

function Probe({ value, observed }: { value: number; observed: (value: number) => void }) {
  useTauriEvent("probe", () => observed(value));
  return null;
}

afterEach(() => {
  vi.clearAllMocks();
  eventMocks.reset();
});

describe("useTauriEvent", () => {
  it("dispatches through the latest handler without resubscribing", () => {
    const observed = vi.fn();
    const view = render(<Probe value={1} observed={observed} />);

    view.rerender(<Probe value={2} observed={observed} />);
    eventMocks.emit(undefined);

    expect(observed).toHaveBeenCalledWith(2);
    expect(eventMocks.listen).toHaveBeenCalledTimes(1);
  });

  it("contains listener registration rejection without an unhandled promise", async () => {
    eventMocks.listen.mockRejectedValueOnce(new Error("listener unavailable"));

    const view = render(<Probe value={1} observed={vi.fn()} />);
    await Promise.resolve();
    await Promise.resolve();

    view.unmount();
    expect(eventMocks.unlisten).not.toHaveBeenCalled();
  });

  it("unsubscribes when the component unmounts", async () => {
    const view = render(<Probe value={1} observed={vi.fn()} />);
    view.unmount();

    await Promise.resolve();
    expect(eventMocks.unlisten).toHaveBeenCalledTimes(1);
  });
});
