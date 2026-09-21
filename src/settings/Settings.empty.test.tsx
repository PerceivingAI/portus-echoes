// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { StepSetupTabs } from "./Settings";

const apiMocks = vi.hoisted(() => ({
  selectActiveProvider: vi.fn(),
  deleteApiKey: vi.fn().mockResolvedValue(undefined),
  downloadModel: vi.fn().mockResolvedValue(undefined),
  getApiKey: vi.fn().mockResolvedValue(""),
  getDiagnosticsState: vi.fn().mockResolvedValue({ last_scan_display: "", snapshot: null }),
  getProviderStatus: vi.fn().mockResolvedValue({
    active_provider: "openai",
    active_configured: false,
  }),
  isHotkeyActive: vi.fn().mockResolvedValue(true),
  isHotkeyRegistered: vi.fn().mockResolvedValue(false),
  listLocalModels: vi.fn().mockResolvedValue([]),
  runDiagnostics: vi.fn().mockResolvedValue({ last_scan_display: "", snapshot: null }),
  resetFocusedHotkeyInput: vi.fn().mockResolvedValue(undefined),
  sendFocusedHotkeyKeyEvent: vi.fn().mockResolvedValue(false),
  saveSettings: vi.fn().mockResolvedValue(undefined),
  setApiKey: vi.fn().mockResolvedValue(undefined),
  setHotkeyCaptureActive: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../lib/constants", () => ({
  LOCAL_MODEL_SLOTS: [null, null],
  OPENAI_MODEL_SLOTS: [
    {
      id: "only-environment-model",
      label: "Only Environment Model",
      route: "completed",
    },
    null,
  ],
  GROQ_MODEL_SLOTS: [null, null],
}));
vi.mock("../lib/api", () => apiMocks);
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    hide: vi.fn().mockResolvedValue(undefined),
    startDragging: vi.fn().mockResolvedValue(undefined),
  }),
}));
vi.mock("../hooks/useTauriEvent", () => ({ useTauriEvent: vi.fn() }));

const settings = {
  active_provider: "openai" as const,
  openai_model: "only-environment-model",
  openai_model_kind: "standard" as const,
  openai_custom_model: "",
  groq_model: "",
  groq_model_kind: "" as const,
  groq_custom_model: "",
  local_model: "",
  local_model_path: "",
  local_model_kind: "" as const,
  local_custom_model_path: "",
  language: "auto",
  hotkey: "Ctrl+Alt+Space",
  onboarding_complete: false,
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("fixed standard model slots", () => {
  it("renders one OpenAI model, one inert empty slot, and Custom", () => {
    render(
      <StepSetupTabs
        settings={settings}
        onSaved={vi.fn()}
        onFinish={vi.fn().mockResolvedValue(undefined)}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: "OpenAI" }));

    const list = document.querySelector("#openai-model-list");
    expect(list?.children).toHaveLength(3);
    const empty = Array.from(list?.children ?? []).find(
      (slot) => slot.getAttribute("aria-hidden") === "true"
    ) as HTMLElement | undefined;
    expect(empty).toBeDefined();

    fireEvent.click(empty as HTMLElement);
    expect(apiMocks.saveSettings).not.toHaveBeenCalled();
  });

  it("renders two inert Local slots when the Local environment list is empty", () => {
    render(
      <StepSetupTabs
        settings={settings}
        onSaved={vi.fn()}
        onFinish={vi.fn().mockResolvedValue(undefined)}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: "Local" }));

    const list = document.querySelector("#local-model-list");
    expect(list?.children).toHaveLength(3);
    expect(
      Array.from(list?.children ?? []).filter(
        (slot) => slot.getAttribute("aria-hidden") === "true"
      )
    ).toHaveLength(2);
    expect(screen.getByText("Custom Path")).toBeTruthy();
  });

  it("renders two inert Groq slots when the Groq environment list is empty", () => {
    render(
      <StepSetupTabs
        settings={settings}
        onSaved={vi.fn()}
        onFinish={vi.fn().mockResolvedValue(undefined)}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: "Groq" }));

    const list = document.querySelector("#groq-model-list");
    expect(list?.children).toHaveLength(3);
    expect(
      Array.from(list?.children ?? []).filter(
        (slot) => slot.getAttribute("aria-hidden") === "true"
      )
    ).toHaveLength(2);
    expect(screen.getByPlaceholderText("Custom Model ID")).toBeTruthy();
  });
});
