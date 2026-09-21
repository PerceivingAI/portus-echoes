/* @vitest-environment jsdom */

import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_SETTINGS } from "../lib/constants";
import type { SettingsNavigationState } from "../lib/types";
import { Onboarding } from "./Onboarding";

const apiMocks = vi.hoisted(() => ({
  completeOnboarding: vi.fn(),
  getSettings: vi.fn(),
  getSettingsNavigation: vi.fn(),
  saveSettings: vi.fn(),
}));

const eventMocks = vi.hoisted(() => {
  let handler: ((event: { payload: unknown }) => void) | undefined;
  const unlisten = vi.fn();
  return {
    listen: vi.fn(
      async (
        event: string,
        next: (event: { payload: unknown }) => void
      ) => {
        if (event === "settings:navigation-requested") handler = next;
        return unlisten;
      }
    ),
    emit(payload: unknown) {
      handler?.({ payload });
    },
    reset() {
      handler = undefined;
      unlisten.mockClear();
    },
    unlisten,
  };
});

vi.mock("../lib/api", () => apiMocks);
vi.mock("@tauri-apps/api/event", () => ({ listen: eventMocks.listen }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ hide: vi.fn() }),
}));
vi.mock("../hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));
vi.mock("./StepWelcome", () => ({
  StepWelcome: ({ onNext }: { onNext: () => void }) => (
    <button type="button" onClick={onNext}>
      Welcome
    </button>
  ),
}));
vi.mock("../settings/Settings", () => ({
  StepSetupTabs: ({
    onFinish,
    initialErrorCode,
    initialTab,
    navigationTab,
  }: {
    onFinish: () => Promise<void>;
    initialErrorCode?: string;
    initialTab?: string;
    navigationTab?: string;
  }) => (
    <div>
      <button type="button" onClick={() => void onFinish()}>
        Settings
      </button>
      <p data-testid="settings-tab">{navigationTab ?? initialTab ?? ""}</p>
      {initialErrorCode ? <p>{initialErrorCode}</p> : null}
    </div>
  ),
}));

beforeEach(() => {
  apiMocks.getSettingsNavigation.mockResolvedValue(null);
});

afterEach(() => {
  cleanup();
  eventMocks.reset();
  vi.clearAllMocks();
});

describe("onboarding startup routing", () => {
  it("shows Welcome when onboarding has not been completed", async () => {
    apiMocks.getSettings.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      onboarding_complete: false,
    });

    render(<Onboarding />);

    expect(await screen.findByRole("button", { name: "Welcome" })).toBeTruthy();
    expect(screen.queryByText("Settings")).toBeNull();
  });

  it("opens Settings without exposing Welcome after onboarding is complete", async () => {
    apiMocks.getSettings.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      onboarding_complete: true,
    });

    render(<Onboarding />);

    expect(await screen.findByText("Settings")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Welcome" })).toBeNull();
  });

  it("retries loading real backend settings on temporary startup failure", async () => {
    apiMocks.getSettings
      .mockRejectedValueOnce("temporary ipc bridge delay")
      .mockResolvedValueOnce({
        ...DEFAULT_SETTINGS,
        onboarding_complete: false,
      });

    render(<Onboarding />);

    expect(await screen.findByRole("button", { name: "Welcome" })).toBeTruthy();
    expect(screen.queryByText("temporary ipc bridge delay")).toBeNull();
  });

  it("completes onboarding through the narrow completion command", async () => {
    apiMocks.getSettings.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      onboarding_complete: false,
    });
    apiMocks.completeOnboarding.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      onboarding_complete: true,
    });
    render(<Onboarding />);

    fireEvent.click(await screen.findByRole("button", { name: "Welcome" }));
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));

    await waitFor(() =>
      expect(apiMocks.completeOnboarding).toHaveBeenCalledTimes(1)
    );
    expect(apiMocks.saveSettings).not.toHaveBeenCalled();
  });

  it("ignores malformed tray navigation while Welcome remains authoritative", async () => {
    apiMocks.getSettings.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      onboarding_complete: false,
    });

    render(<Onboarding />);
    expect(await screen.findByRole("button", { name: "Welcome" })).toBeTruthy();

    act(() => {
      eventMocks.emit(null);
      eventMocks.emit({ tab: "diagnostics", revision: "invalid" });
      eventMocks.emit({ tab: "local", revision: 1 });
    });

    expect(screen.getByRole("button", { name: "Welcome" })).toBeTruthy();
    expect(screen.queryByText("Settings")).toBeNull();
  });

  it("bypasses Welcome immediately for a live tray Diagnostics request", async () => {
    let resolveSettings!: (value: typeof DEFAULT_SETTINGS) => void;
    apiMocks.getSettings.mockReturnValue(
      new Promise<typeof DEFAULT_SETTINGS>((resolve) => {
        resolveSettings = resolve;
      })
    );

    render(<Onboarding />);
    await waitFor(() => expect(eventMocks.listen).toHaveBeenCalledTimes(1));

    act(() => {
      eventMocks.emit({ tab: "diagnostics", revision: 1 });
    });

    await act(async () => {
      resolveSettings({ ...DEFAULT_SETTINGS, onboarding_complete: false });
      await Promise.resolve();
    });

    expect(await screen.findByText("Settings")).toBeTruthy();
    expect(screen.getByTestId("settings-tab").textContent).toBe("diagnostics");
    expect(screen.queryByRole("button", { name: "Welcome" })).toBeNull();
  });

  it("recovers a tray request that happened before the listener was ready", async () => {
    apiMocks.getSettingsNavigation.mockResolvedValue({
      tab: "diagnostics",
      revision: 1,
    });
    apiMocks.getSettings.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      onboarding_complete: false,
    });

    render(<Onboarding />);

    expect(await screen.findByText("Settings")).toBeTruthy();
    expect(screen.getByTestId("settings-tab").textContent).toBe("diagnostics");
    expect(screen.queryByRole("button", { name: "Welcome" })).toBeNull();
  });

  it("rejects a stale navigation snapshot after a newer live tray request", async () => {
    let resolveNavigation!: (value: SettingsNavigationState | null) => void;
    apiMocks.getSettingsNavigation.mockReturnValue(
      new Promise<SettingsNavigationState | null>((resolve) => {
        resolveNavigation = resolve;
      })
    );
    apiMocks.getSettings.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      onboarding_complete: true,
    });

    render(<Onboarding />);
    await waitFor(() => expect(eventMocks.listen).toHaveBeenCalledTimes(1));

    act(() => {
      eventMocks.emit({ tab: "settings", revision: 2 });
    });
    expect(await screen.findByText("Settings")).toBeTruthy();
    expect(screen.getByTestId("settings-tab").textContent).toBe("settings");

    await act(async () => {
      resolveNavigation({ tab: "diagnostics", revision: 1 });
      await Promise.resolve();
    });

    expect(screen.getByTestId("settings-tab").textContent).toBe("settings");
  });
});
