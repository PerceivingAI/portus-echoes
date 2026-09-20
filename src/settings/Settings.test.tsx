// @vitest-environment jsdom

import { useState } from "react";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_SETTINGS } from "../lib/constants";
import type {
  AppSettings,
  DiagnosticsStorageData,
  LocalModelKind,
  SettingsNavigationTab,
  UserErrorCode,
} from "../lib/types";
import { USER_ERROR_MESSAGES } from "../lib/errors";
import { StepSetupTabs } from "./Settings";

const modelMapMocks = vi.hoisted(() => ({
  local: [
    {
      downloadPath: "https://models.example.test/local-one.bin",
      label: "Local Environment One",
    },
    {
      downloadPath: "https://models.example.test/local-two.bin",
      label: "Local Environment Two",
    },
  ],
  openai: [
    {
      id: "openai-environment-one",
      label: "OpenAI Environment One",
      route: "completed",
    },
    {
      id: "openai-environment-two",
      label: "OpenAI Environment Two",
      route: "live",
    },
  ],
  groq: [
    {
      id: "groq-environment-one",
      label: "Groq Environment One",
      route: "completed",
    },
    {
      id: "groq-environment-two",
      label: "Groq Environment Two",
      route: "completed",
    },
  ],
}));

vi.mock("../lib/constants", () => ({
  LOCAL_MODEL_SLOTS: modelMapMocks.local,
  OPENAI_MODEL_SLOTS: modelMapMocks.openai,
  GROQ_MODEL_SLOTS: modelMapMocks.groq,
  DEFAULT_SETTINGS: {
    active_provider: "local",
    openai_model: modelMapMocks.openai[0].id,
    openai_model_kind: "standard",
    openai_custom_model: "",
    groq_model: modelMapMocks.groq[0].id,
    groq_model_kind: "standard",
    groq_custom_model: "",
    local_model_path: "",
    local_model_kind: "standard",
    local_custom_model_path: "",
    language: "auto",
    hotkey: "Ctrl+Alt+Space",
    cloud_recording_limit_secs: 1800,
    onboarding_complete: false,
  },
}));

const apiMocks = vi.hoisted(() => ({
  cancelDownload: vi.fn(),
  completeOnboarding: vi.fn(),
  deleteApiKey: vi.fn(),
  deleteModel: vi.fn(),
  downloadModel: vi.fn(),
  getApiKey: vi.fn(),
  getDiagnosticsState: vi.fn(),
  getProviderStatus: vi.fn(),
  getSettings: vi.fn(),
  isHotkeyActive: vi.fn(),
  isHotkeyRegistered: vi.fn(),
  listLocalModels: vi.fn(),
  resetFocusedHotkeyInput: vi.fn(),
  sendFocusedHotkeyKeyEvent: vi.fn(),
  runDiagnostics: vi.fn(),
  saveSettings: vi.fn(),
  selectActiveProvider: vi.fn(),
  selectGroqCustomModel: vi.fn(),
  selectGroqModel: vi.fn(),
  selectLocalCustomModel: vi.fn(),
  selectLocalModel: vi.fn(),
  selectOpenaiCustomModel: vi.fn(),
  selectOpenaiModel: vi.fn(),
  setApiKey: vi.fn(),
  setGroqCustomModel: vi.fn(),
  setHotkey: vi.fn(),
  setHotkeyCaptureActive: vi.fn(),
  setLocalCustomModelPath: vi.fn(),
  setOpenaiCustomModel: vi.fn(),
}));

const dialogMocks = vi.hoisted(() => ({
  open: vi.fn(),
}));

const windowMocks = vi.hoisted(() => ({
  hide: vi.fn().mockResolvedValue(undefined),
  startDragging: vi.fn().mockResolvedValue(undefined),
}));

const tauriEventMocks = vi.hoisted(() => {
  const handlers = new Map<string, (payload: unknown) => void>();
  return {
    clear: () => handlers.clear(),
    emit: (event: string, payload: unknown) => {
      const handler = handlers.get(event);
      if (!handler) throw new Error(`No handler registered for ${event}`);
      handler(payload);
    },
    register: (event: string, handler: (payload: unknown) => void) => {
      handlers.set(event, handler);
    },
  };
});

vi.mock("../lib/api", () => apiMocks);
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: dialogMocks.open }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => windowMocks,
}));
vi.mock("../hooks/useTauriEvent", () => ({
  useTauriEvent: (
    event: string,
    handler: (payload: unknown) => void
  ) => tauriEventMocks.register(event, handler),
}));

const localInfos = [
  {
    download_path: modelMapMocks.local[0].downloadPath,
    path: "C:\\PortusEchoes\\models\\environment-one.bin",
    downloaded: false,
    size_bytes: null,
  },
  {
    download_path: modelMapMocks.local[1].downloadPath,
    path: "C:\\PortusEchoes\\models\\environment-two.bin",
    downloaded: true,
    size_bytes: 1_500_000_000,
  },
];

const completedDiagnostics: DiagnosticsStorageData = {
  last_scan_display: "21 Mar 2026",
  snapshot: {
    local: {
      passed: "pass",
      model_path: "validated",
      model_file: "validated",
    },
    cloud: {
      passed: "pass",
      openai_api_key: "validated",
      openai_model: "validated",
      groq_api_key: "none_not_valid",
      groq_model: "none_not_valid",
    },
    microphone: {
      passed: "pass",
      name: "Integration Test Microphone",
      specs: "48000 Hz, 2 Channels",
    },
    transcription_access: {
      passed: "pass",
      function: "send_input",
      access_type: "cursor",
    },
  },
};

let backendSettings: AppSettings = DEFAULT_SETTINGS;
let lastPersistedSettings: AppSettings | null = null;

const commandResult = (
  changes: Partial<AppSettings>
): Promise<AppSettings> => {
  const next = { ...backendSettings, ...changes };
  backendSettings = next;
  lastPersistedSettings = next;
  return Promise.resolve(next);
};

function SettingsHarness({
  initialSettings,
  onSaved,
  onFinish = vi.fn().mockResolvedValue(undefined),
  navigationTab,
}: {
  initialSettings: AppSettings;
  onSaved?: (settings: AppSettings) => void;
  onFinish?: () => Promise<void>;
  navigationTab?: SettingsNavigationTab;
}) {
  const [settings, setSettings] = useState(initialSettings);
  if (!lastPersistedSettings) backendSettings = settings;

  return (
    <StepSetupTabs
      settings={settings}
      onSaved={(next) => {
        onSaved?.(next);
        setSettings(next);
      }}
      onFinish={onFinish}
      navigationTab={navigationTab}
    />
  );
}

const localSettings = (
  localModelPath = "",
  localModelKind: LocalModelKind = "",
  activeProvider: AppSettings["active_provider"] = "local"
): AppSettings => ({
  ...DEFAULT_SETTINGS,
  active_provider: activeProvider,
  local_model_path: localModelPath,
  local_model_kind: localModelKind,
  local_custom_model_path:
    localModelKind === "custom" ? localModelPath : "",
});

const customPathInput = () =>
  screen.getByPlaceholderText<HTMLInputElement>(
    "Custom path to a local model file (.bin)"
  );

const localModelOption = (label: string) => {
  const option = screen.getByText(label).closest(".local-model-option");
  expect(option).not.toBeNull();
  return option as HTMLElement;
};

const expectLocalModelSelected = (option: HTMLElement) => {
  expect(option.firstElementChild?.classList.contains("bg-accent")).toBe(true);
};

const expectLocalModelUnselected = (option: HTMLElement) => {
  expect(option.firstElementChild?.classList.contains("bg-accent")).toBe(false);
};

const cloudModelOption = (label: string) => {
  const option = screen.getByText(label).closest(".cloud-model-option");
  expect(option).not.toBeNull();
  return option as HTMLElement;
};

const expectCloudModelSelected = (option: HTMLElement) => {
  expect(option.firstElementChild?.classList.contains("bg-accent")).toBe(true);
};

const expectCloudModelUnselected = (option: HTMLElement) => {
  expect(option.firstElementChild?.classList.contains("bg-accent")).toBe(false);
};

const lastSavedSettings = (): AppSettings => {
  if (!lastPersistedSettings) {
    throw new Error("No settings command has persisted a value");
  }
  return lastPersistedSettings;
};

beforeEach(() => {
  backendSettings = DEFAULT_SETTINGS;
  lastPersistedSettings = null;
  apiMocks.cancelDownload.mockResolvedValue(undefined);
  apiMocks.deleteApiKey.mockResolvedValue(undefined);
  apiMocks.deleteModel.mockResolvedValue(localSettings());
  apiMocks.downloadModel.mockResolvedValue(undefined);
  apiMocks.getApiKey.mockResolvedValue("");
  apiMocks.getProviderStatus.mockResolvedValue({
    openai_configured: false,
    groq_configured: false,
    local_configured: false,
    active_provider: "local",
    active_configured: false,
  });
  apiMocks.listLocalModels.mockResolvedValue(localInfos);
  apiMocks.isHotkeyActive.mockResolvedValue(true);
  apiMocks.isHotkeyRegistered.mockResolvedValue(false);
  apiMocks.resetFocusedHotkeyInput.mockResolvedValue(undefined);
  apiMocks.sendFocusedHotkeyKeyEvent.mockResolvedValue(false);
  apiMocks.setHotkeyCaptureActive.mockResolvedValue(undefined);
  apiMocks.selectLocalModel.mockImplementation((path: string) =>
    commandResult({
      local_model_path: path,
      local_model_kind: "standard",
    })
  );
  apiMocks.getDiagnosticsState.mockResolvedValue({ last_scan_display: "", snapshot: null });
  apiMocks.runDiagnostics.mockResolvedValue({ last_scan_display: "", snapshot: null });
  apiMocks.saveSettings.mockResolvedValue(undefined);
  apiMocks.selectOpenaiModel.mockImplementation((model: string) =>
    commandResult({ openai_model: model, openai_model_kind: "standard" })
  );
  apiMocks.selectOpenaiCustomModel.mockImplementation(() =>
    commandResult({
      openai_model: backendSettings.openai_custom_model,
      openai_model_kind: "custom",
    })
  );
  apiMocks.selectGroqModel.mockImplementation((model: string) =>
    commandResult({ groq_model: model, groq_model_kind: "standard" })
  );
  apiMocks.selectGroqCustomModel.mockImplementation(() =>
    commandResult({
      groq_model: backendSettings.groq_custom_model,
      groq_model_kind: "custom",
    })
  );
  apiMocks.selectLocalCustomModel.mockImplementation(() =>
    commandResult({
      local_model_path: backendSettings.local_custom_model_path,
      local_model_kind: "custom",
    })
  );
  apiMocks.setOpenaiCustomModel.mockImplementation((model: string) =>
    commandResult({
      openai_custom_model: model,
      ...(backendSettings.openai_model_kind === "custom"
        ? { openai_model: model }
        : {}),
    })
  );
  apiMocks.setGroqCustomModel.mockImplementation((model: string) =>
    commandResult({
      groq_custom_model: model,
      ...(backendSettings.groq_model_kind === "custom"
        ? { groq_model: model }
        : {}),
    })
  );
  apiMocks.setLocalCustomModelPath.mockImplementation((path: string) =>
    commandResult({
      local_custom_model_path: path,
      ...(backendSettings.local_model_kind === "custom"
        ? { local_model_path: path }
        : {}),
    })
  );
  apiMocks.setHotkey.mockImplementation((hotkey: string) =>
    commandResult({ hotkey })
  );
  apiMocks.setApiKey.mockResolvedValue(undefined);
  apiMocks.selectActiveProvider.mockImplementation(
    (provider: AppSettings["active_provider"]) =>
      commandResult({ active_provider: provider })
  );
});

afterEach(() => {
  cleanup();
  tauriEventMocks.clear();
  vi.clearAllMocks();
  vi.useRealTimers();
});

describe("Phase 1 Settings header", () => {
  it("keeps the title position and places navigation below the header", () => {
    render(
      <SettingsHarness
        initialSettings={{ ...DEFAULT_SETTINGS, active_provider: "openai" }}
      />
    );

    const title = screen.getByRole("heading", {
      level: 1,
      name: "Portus Echoes",
    });
    const header = title.closest("header");
    expect(header).not.toBeNull();
    expect(title.className).toContain(
      "text-xl font-semibold tracking-tight text-slate-100"
    );
    expect(header?.className).toContain("pl-6");
    expect(header?.className).toContain("pr-8");
    expect(header?.className).toContain("py-4");

    const navigation = screen.getByRole("navigation");
    expect(header?.contains(navigation)).toBe(false);
    expect(
      (header?.compareDocumentPosition(navigation) ?? 0) &
        Node.DOCUMENT_POSITION_FOLLOWING
    ).not.toBe(0);
    expect(screen.getAllByRole("button", { name: "Close" })).toHaveLength(1);
  });

  it("flushes through the header X before hiding and preserves dragging", async () => {
    const onFinish = vi.fn().mockResolvedValue(undefined);
    render(
      <SettingsHarness
        initialSettings={{ ...DEFAULT_SETTINGS, active_provider: "openai" }}
        onFinish={onFinish}
      />
    );

    const close = screen.getByRole("button", { name: "Close" });
    const header = close.closest("header");
    expect(header).not.toBeNull();

    fireEvent.mouseDown(close, { button: 0 });
    expect(windowMocks.startDragging).not.toHaveBeenCalled();

    fireEvent.click(close);
    await waitFor(() => expect(onFinish).toHaveBeenCalledTimes(1));
    expect(windowMocks.hide).toHaveBeenCalledTimes(1);
    expect(apiMocks.saveSettings).not.toHaveBeenCalled();

    fireEvent.mouseDown(header as HTMLElement, { button: 0 });
    expect(windowMocks.startDragging).toHaveBeenCalledTimes(1);
  });
});

describe("Phase 2 Settings sidebar", () => {
  it("renders the required navigation order with exact Lucide icons", () => {
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

    const navigation = screen.getByRole("navigation", {
      name: "Settings sections",
    });
    const entries = within(navigation).getAllByRole("button");
    const expected = [
      ["Settings", "lucide-settings"],
      ["Local", "lucide-cpu"],
      ["OpenAI", "lucide-cloud"],
      ["Groq", "lucide-zap"],
      ["Diagnostics", "lucide-shield-check"],
    ] as const;

    expect(entries).toHaveLength(expected.length);
    expected.forEach(([label, iconClass], index) => {
      const entry = entries[index];
      const icon = entry.querySelector("svg");
      expect(entry.textContent).toBe(label);
      expect(icon).not.toBeNull();
      expect(icon?.classList.contains(iconClass)).toBe(true);
    });
  });

  it("selects every existing tab without mutating Settings", () => {
    const onSaved = vi.fn();
    const { container } = render(
      <SettingsHarness
        initialSettings={{ ...DEFAULT_SETTINGS, active_provider: "openai" }}
        onSaved={onSaved}
      />
    );
    const navigation = screen.getByRole("navigation", {
      name: "Settings sections",
    });
    const entries = [
      ["settings", "Settings"],
      ["local", "Local"],
      ["openai", "OpenAI"],
      ["groq", "Groq"],
      ["diagnostics", "Diagnostics"],
    ] as const;

    entries.forEach(([id, label]) => {
      const entry = within(navigation).getByRole("button", { name: label });
      fireEvent.click(entry);
      expect(container.querySelector(`#pane-${id}`)).not.toBeNull();
      expect(entry.getAttribute("aria-current")).toBe("page");
      expect(entry.className).toContain("active");
      expect(
        within(navigation)
          .getAllByRole("button")
          .filter((button) => button.getAttribute("aria-current") === "page")
      ).toHaveLength(1);
    });

    expect(onSaved).not.toHaveBeenCalled();
    expect(apiMocks.saveSettings).not.toHaveBeenCalled();
    expect(apiMocks.setApiKey).not.toHaveBeenCalled();
  });

  it("loads Diagnostics once and never reloads or runs it during Local -> Diagnostics -> OpenAI -> Diagnostics navigation", async () => {
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
    const diagnostics = screen.getByRole("button", { name: "Diagnostics" });
    const openai = screen.getByRole("button", { name: "OpenAI" });

    await waitFor(() =>
      expect(apiMocks.getDiagnosticsState).toHaveBeenCalledTimes(1)
    );
    expect(apiMocks.runDiagnostics).not.toHaveBeenCalled();

    fireEvent.click(diagnostics);
    expect(apiMocks.getDiagnosticsState).toHaveBeenCalledTimes(1);
    expect(apiMocks.runDiagnostics).not.toHaveBeenCalled();

    fireEvent.click(openai);
    fireEvent.click(diagnostics);
    expect(apiMocks.getDiagnosticsState).toHaveBeenCalledTimes(1);
    expect(apiMocks.runDiagnostics).not.toHaveBeenCalled();
  });

  it("still accepts the mount-time cached snapshot after an explicit Diagnostics run fails", async () => {
    let resolveInitialLoad!: (value: DiagnosticsStorageData) => void;
    apiMocks.getDiagnosticsState.mockImplementationOnce(
      () =>
        new Promise<DiagnosticsStorageData>((resolve) => {
          resolveInitialLoad = resolve;
        })
    );
    apiMocks.runDiagnostics.mockRejectedValueOnce("worker_failed");
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

    fireEvent.click(screen.getByRole("button", { name: "Diagnostics" }));
    fireEvent.click(screen.getByRole("button", { name: "Run Diagnostics" }));

    await waitFor(() => expect(apiMocks.runDiagnostics).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getByText("Unable to proceed")).toBeTruthy());

    await act(async () => {
      resolveInitialLoad(completedDiagnostics);
    });

    expect(screen.getByText("Unable to proceed")).toBeTruthy();

    await new Promise((resolve) => setTimeout(resolve, 1600));
    expect(screen.getByText("Name: Integration Test Microphone")).toBeTruthy();
  });

  it("does not let a stale mount-time Diagnostics cache load overwrite a completed explicit run", async () => {
    let resolveInitialLoad!: (value: DiagnosticsStorageData) => void;
    apiMocks.getDiagnosticsState.mockImplementationOnce(
      () =>
        new Promise<DiagnosticsStorageData>((resolve) => {
          resolveInitialLoad = resolve;
        })
    );
    apiMocks.runDiagnostics.mockResolvedValueOnce(completedDiagnostics);
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

    fireEvent.click(screen.getByRole("button", { name: "Diagnostics" }));
    fireEvent.click(screen.getByRole("button", { name: "Run Diagnostics" }));

    await waitFor(() => expect(apiMocks.runDiagnostics).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(screen.getByText("Name: Integration Test Microphone")).toBeTruthy()
    );

    await act(async () => {
      resolveInitialLoad({ last_scan_display: "", snapshot: null });
    });

    expect(screen.getByText("Name: Integration Test Microphone")).toBeTruthy();
    expect(screen.queryByText("Name: -")).toBeNull();
  });

  it("runs Diagnostics only from the Run Diagnostics button and replaces the frontend snapshot once", async () => {
    apiMocks.runDiagnostics.mockResolvedValueOnce(completedDiagnostics);
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

    await waitFor(() =>
      expect(apiMocks.getDiagnosticsState).toHaveBeenCalledTimes(1)
    );
    fireEvent.click(screen.getByRole("button", { name: "Diagnostics" }));
    expect(screen.getByText("Name: -")).toBeTruthy();
    expect(apiMocks.runDiagnostics).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Run Diagnostics" }));

    await waitFor(() => expect(apiMocks.runDiagnostics).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(screen.getByText("Name: Integration Test Microphone")).toBeTruthy()
    );
    expect(apiMocks.getDiagnosticsState).toHaveBeenCalledTimes(1);
    expect(apiMocks.runDiagnostics).toHaveBeenCalledTimes(1);
  });

  it("shows Unable to proceed when Diagnostics fails while preserving the previous snapshot", async () => {
    apiMocks.getDiagnosticsState.mockResolvedValueOnce(completedDiagnostics);
    apiMocks.runDiagnostics.mockRejectedValueOnce("persistence_failed");
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

    fireEvent.click(screen.getByRole("button", { name: "Diagnostics" }));
    await waitFor(() =>
      expect(screen.getByText("Name: Integration Test Microphone")).toBeTruthy()
    );
    fireEvent.click(screen.getByRole("button", { name: "Run Diagnostics" }));

    await waitFor(() => expect(apiMocks.runDiagnostics).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getByText("Unable to proceed")).toBeTruthy());
    expect(screen.getByText("OpenAI Model: -")).toBeTruthy();
    expect(screen.getByText("Name: -")).toBeTruthy();
    expect(screen.queryByText("Ready")).toBeNull();
    expect(screen.queryByText("Name: Integration Test Microphone")).toBeNull();
  });

  it("does not launch another Diagnostics suite when settings change during a pending run", async () => {
    let resolveRun!: (value: DiagnosticsStorageData) => void;
    apiMocks.getDiagnosticsState.mockResolvedValueOnce(completedDiagnostics);
    apiMocks.runDiagnostics.mockImplementationOnce(
      () =>
        new Promise<DiagnosticsStorageData>((resolve) => {
          resolveRun = resolve;
        })
    );
    const { container } = render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

    fireEvent.click(screen.getByRole("button", { name: "Diagnostics" }));
    await waitFor(() => expect(screen.getByText("OpenAI Model: Validated")).toBeTruthy());
    expect(container.querySelectorAll("[data-diagnostic-group-icon]")).toHaveLength(4);

    fireEvent.click(screen.getByRole("button", { name: "Run Diagnostics" }));
    await waitFor(() => expect(apiMocks.runDiagnostics).toHaveBeenCalledTimes(1));
    expect(screen.getByText("Running...")).toBeTruthy();
    expect(screen.getByText("OpenAI Model: -")).toBeTruthy();
    expect(container.querySelectorAll("[data-diagnostic-group-icon]")).toHaveLength(0);

    fireEvent.click(screen.getByRole("button", { name: "OpenAI" }));
    fireEvent.click(cloudModelOption("OpenAI Environment Two"));
    await waitFor(() =>
      expect(apiMocks.selectOpenaiModel).toHaveBeenCalledWith(
        "openai-environment-two"
      )
    );
    expect(apiMocks.runDiagnostics).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Diagnostics" }));
    expect(apiMocks.getDiagnosticsState).toHaveBeenCalledTimes(1);
    expect(apiMocks.runDiagnostics).toHaveBeenCalledTimes(1);
    expect(screen.getByText("Running...")).toBeTruthy();

    await act(async () => {
      resolveRun(completedDiagnostics);
    });
    await waitFor(() =>
      expect(screen.getByText("Name: Integration Test Microphone")).toBeTruthy()
    );
    expect(apiMocks.runDiagnostics).toHaveBeenCalledTimes(1);
  });

  it("keeps navigation fixed outside the scrolling content column", () => {
    const { container } = render(
      <SettingsHarness initialSettings={DEFAULT_SETTINGS} />
    );
    const header = screen.getByRole("heading", {
      level: 1,
      name: "Portus Echoes",
    }).closest("header");
    const body = header?.nextElementSibling;
    const sidebar = container.querySelector("aside");
    const navigation = screen.getByRole("navigation", {
      name: "Settings sections",
    });
    const content = container.querySelector("main#pane-container");

    expect(body).not.toBeNull();
    expect(sidebar).not.toBeNull();
    expect(content).not.toBeNull();
    expect(body?.contains(sidebar)).toBe(true);
    expect(body?.contains(content)).toBe(true);
    expect(sidebar?.contains(navigation)).toBe(true);
    expect(sidebar?.contains(content)).toBe(false);
    expect(sidebar?.className).not.toContain("overflow-y-auto");
    expect(content?.className).toContain("overflow-y-auto");
  });

  it("moves the sole Start control and its warning into the sidebar", () => {
    const { container } = render(
      <SettingsHarness initialSettings={DEFAULT_SETTINGS} />
    );
    const sidebar = container.querySelector("aside");
    expect(sidebar).not.toBeNull();

    const start = within(sidebar as HTMLElement).getByRole<HTMLButtonElement>(
      "button",
      { name: "Start" }
    );
    expect(start.disabled).toBe(true);
    expect(screen.getAllByRole("button", { name: "Start" })).toHaveLength(1);
    expect(
      within(sidebar as HTMLElement).getByText(
        "Configure at least one provider."
      )
    ).not.toBeNull();
    expect(container.querySelector("footer")).toBeNull();
  });

  it("preserves the Start finish and busy behavior after relocation", async () => {
    const onFinish = vi.fn().mockResolvedValue(undefined);
    render(
      <SettingsHarness
        initialSettings={localSettings(
          "C:\\PortusEchoes\\models\\custom.bin",
          "custom"
        )}
        onFinish={onFinish}
      />
    );
    const start = screen.getByRole<HTMLButtonElement>("button", {
      name: "Start",
    });

    expect(start.disabled).toBe(false);
    fireEvent.click(start);

    await waitFor(() => expect(onFinish).toHaveBeenCalledTimes(1));
    expect(windowMocks.hide).toHaveBeenCalledTimes(1);
    expect(start.disabled).toBe(false);
    expect(apiMocks.setApiKey).not.toHaveBeenCalled();
  });

  it("confines dragging to the Settings header", async () => {
    const { container } = render(
      <SettingsHarness initialSettings={localSettings()} />
    );
    const close = screen.getByRole("button", { name: "Close" });
    const header = close.closest("header");
    const body = header?.nextElementSibling as HTMLElement;
    const sidebar = container.querySelector("aside") as HTMLElement;
    const navigation = screen.getByRole("navigation", {
      name: "Settings sections",
    });
    const content = container.querySelector("main#pane-container") as HTMLElement;
    const start = screen.getByRole("button", { name: "Start" });
    const dragRegions = Array.from(
      container.querySelectorAll("[data-tauri-drag-region]")
    );

    expect(header).not.toBeNull();
    expect(dragRegions.length).toBeGreaterThan(0);
    expect(dragRegions.every((region) => header?.contains(region))).toBe(true);

    fireEvent.mouseDown(header as HTMLElement, { button: 0 });
    expect(windowMocks.startDragging).toHaveBeenCalledTimes(1);
    windowMocks.startDragging.mockClear();

    [body, sidebar, navigation, content, start].forEach((surface) => {
      fireEvent.mouseDown(surface, { button: 0 });
    });
    expect(windowMocks.startDragging).not.toHaveBeenCalled();

    const deleteModel = await screen.findByRole("button", {
      name: "Delete model",
    });
    fireEvent.click(deleteModel);
    const modalBackdrop = screen.getByRole("dialog").parentElement;
    expect(modalBackdrop).not.toBeNull();
    fireEvent.mouseDown(modalBackdrop as HTMLElement, { button: 0 });
    expect(windowMocks.startDragging).not.toHaveBeenCalled();
  });
});

describe("CLOUD_PLAN Phase 4 runtime transcription errors", () => {
  const runtimeErrorCases: ReadonlyArray<[UserErrorCode, string]> = [
    ["credential_storage", USER_ERROR_MESSAGES.credential_storage],
    ["network_connection", USER_ERROR_MESSAGES.network_connection],
    ["cloud_service", USER_ERROR_MESSAGES.cloud_service],
    ["local_model_not_found", USER_ERROR_MESSAGES.local_model_not_found],
    ["api_key_rejected", USER_ERROR_MESSAGES.api_key_rejected],
    ["rate_limit", USER_ERROR_MESSAGES.rate_limit],
    ["recording_too_large", USER_ERROR_MESSAGES.recording_too_large],
    ["delivery", USER_ERROR_MESSAGES.delivery],
    ["unexpected", USER_ERROR_MESSAGES.unexpected],
  ];

  it.each(runtimeErrorCases)(
    "routes %s through the existing Settings AlertBanner",
    (code, message) => {
      render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

      act(() => {
        tauriEventMocks.emit("transcription:error", { code });
      });

      const banner = screen.getByText(message);
      expect(banner.tagName).toBe("P");
      expect(banner.className).toBe("text-xs text-red-400 bg-transparent");
      expect(windowMocks.hide).not.toHaveBeenCalled();
    }
  );

  it("routes general application codes through the same existing AlertBanner", () => {
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

    act(() => {
      tauriEventMocks.emit("app:error", { code: "invalid_shortcut" });
    });

    const banner = screen.getByText(USER_ERROR_MESSAGES.invalid_shortcut);
    expect(banner.tagName).toBe("P");
    expect(banner.className).toBe("text-xs text-red-400 bg-transparent");
    expect(windowMocks.hide).not.toHaveBeenCalled();
  });

  it("maps malformed payloads to unexpected without rendering raw fields", () => {
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

    act(() => {
      tauriEventMocks.emit("transcription:error", {
        code: "not_a_real_code",
        message: "raw provider failure",
        status: 500,
      });
    });

    expect(screen.getByText(USER_ERROR_MESSAGES.unexpected)).not.toBeNull();
    expect(screen.queryByText("raw provider failure")).toBeNull();
    expect(screen.queryByText("500")).toBeNull();
  });

  it("maps malformed preload-error payloads to unexpected instead of throwing", () => {
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

    act(() => {
      tauriEventMocks.emit("model:preload:error", null);
    });

    expect(screen.getByText(USER_ERROR_MESSAGES.unexpected)).not.toBeNull();
  });

  it("shows no runtime error when no error event is received", () => {
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

    for (const message of Object.values(USER_ERROR_MESSAGES)) {
      expect(screen.queryByText(message)).toBeNull();
    }
  });

  it("clears the active error banner when a new recording attempt begins", () => {
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

    act(() => {
      tauriEventMocks.emit("transcription:error", { code: "unexpected" });
    });

    expect(screen.getByText(USER_ERROR_MESSAGES.unexpected)).not.toBeNull();

    act(() => {
      tauriEventMocks.emit("recording:state-changed", {
        phase: "preparing",
        revision: 2,
      });
    });

    expect(screen.queryByText(USER_ERROR_MESSAGES.unexpected)).toBeNull();
  });
  it("displays Processing... and On Clipboard! banners during clipboard delivery and clears on new recording", () => {
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

    // 1. Processing status
    act(() => {
      tauriEventMocks.emit("delivery:clipboard-status", { status: "processing" });
    });
    expect(screen.getByText("Processing...")).not.toBeNull();
    const processingEl = screen.getByTestId("status-banner");
    expect(processingEl.className).toContain("text-slate-400");

    // 2. Copied status
    act(() => {
      tauriEventMocks.emit("delivery:clipboard-status", { status: "copied" });
    });
    expect(screen.queryByText("Processing...")).toBeNull();
    expect(screen.getByText("On Clipboard!")).not.toBeNull();
    const copiedEl = screen.getByTestId("status-banner");
    expect(copiedEl.className).toContain("text-accent");

    // 3. Cleared on new recording start
    act(() => {
      tauriEventMocks.emit("recording:state-changed", {
        phase: "preparing",
        revision: 3,
      });
    });
    expect(screen.queryByText("On Clipboard!")).toBeNull();
    expect(screen.queryByTestId("status-banner")).toBeNull();
  });
});

describe("Phase 4 cloud provider content", () => {
  it.each([
    {
      provider: "OpenAI",
      paneId: "pane-openai",
      listId: "openai-model-list",
      subtitleLine1: "Select an OpenAI model and add your API Key.",
      subtitleLine2: "For additional OpenAI models, add a Custom Model ID.",
    },
    {
      provider: "Groq",
      paneId: "pane-groq",
      listId: "groq-model-list",
      subtitleLine1: "Select a Groq model and add your API Key.",
      subtitleLine2: "For additional Groq models, add a Custom Model ID.",
    },
  ] as const)(
    "uses the grounded content order and underlined fields for $provider",
    ({ provider, paneId, listId, subtitleLine1, subtitleLine2 }) => {
      const { container } = render(
        <SettingsHarness initialSettings={DEFAULT_SETTINGS} />
      );
      fireEvent.click(screen.getByRole("button", { name: provider }));

      const pane = container.querySelector<HTMLElement>(`#${paneId}`);
      expect(pane).not.toBeNull();
      const paneQueries = within(pane as HTMLElement);
      const title = paneQueries.getByRole("heading", {
        level: 2,
        name: "Cloud Provider",
      });
      const subtitle = paneQueries.getByText(
        (_, el) => el?.tagName.toLowerCase() === "p" && (el.textContent?.includes(subtitleLine1) ?? false)
      );
      expect(subtitle).not.toBeNull();
      expect(subtitle.textContent).toContain(subtitleLine2);
      const modelsLabel = paneQueries.getByText("Models");
      const modelList = pane?.querySelector<HTMLElement>(`#${listId}`);
      const apiKeyLabel = paneQueries.getByText("API Key");
      const customModel = paneQueries.getByPlaceholderText<HTMLInputElement>(
        "Custom Model ID"
      );
      const apiKey =
        paneQueries.getByPlaceholderText<HTMLInputElement>("Your API Key");

      expect(title.className).toBe(
        "text-sm font-semibold text-slate-200 tracking-wide"
      );
      expect(subtitle.className).toBe("text-xs text-slate-400 mt-0.5");
      expect(
        subtitle.compareDocumentPosition(modelsLabel) &
          Node.DOCUMENT_POSITION_FOLLOWING
      ).not.toBe(0);
      expect(
        modelsLabel.compareDocumentPosition(modelList as HTMLElement) &
          Node.DOCUMENT_POSITION_FOLLOWING
      ).not.toBe(0);
      expect(
        (modelList as HTMLElement).compareDocumentPosition(apiKeyLabel) &
          Node.DOCUMENT_POSITION_FOLLOWING
      ).not.toBe(0);
      expect(
        apiKeyLabel.compareDocumentPosition(apiKey) &
          Node.DOCUMENT_POSITION_FOLLOWING
      ).not.toBe(0);

      expect(modelList?.children).toHaveLength(3);
      expect(modelList?.className).toContain("flex flex-col");
      expect(modelList?.querySelector(".model-card")).toBeNull();
      expect(customModel.getAttribute("placeholder")).toBe("Custom Model ID");
      expect(customModel.closest(".cloud-model-option")).not.toBeNull();
      expect(customModel.closest(".model-card")).toBeNull();
      expect(customModel.parentElement?.className).toContain("border-b");
      expect(apiKey.getAttribute("placeholder")).toBe("Your API Key");
      expect(apiKey.parentElement?.className).toContain("border-b");
      expect(apiKey.closest(".hud-border")).toBeNull();
    }
  );

  it.each([
    { provider: "OpenAI", providerId: "openai" },
    { provider: "Groq", providerId: "groq" },
  ] as const)(
    "preserves the $provider API Key placeholder, blur save, and visibility toggle",
    async ({ provider, providerId }) => {
      const { container } = render(
        <SettingsHarness initialSettings={DEFAULT_SETTINGS} />
      );
      fireEvent.click(screen.getByRole("button", { name: provider }));
      await waitFor(() =>
        expect(apiMocks.getApiKey).toHaveBeenCalledWith(providerId)
      );

      const pane = container.querySelector<HTMLElement>(
        `#pane-${providerId}`
      ) as HTMLElement;
      const paneQueries = within(pane);
      const apiKey =
        paneQueries.getByPlaceholderText<HTMLInputElement>("Your API Key");
      const visibility = paneQueries.getByRole("button", {
        name: "Toggle API Key visibility",
      });

      expect(apiKey.getAttribute("placeholder")).toBe("Your API Key");
      expect(apiKey.type).toBe("password");
      fireEvent.change(apiKey, { target: { value: "  provider-secret  " } });
      fireEvent.blur(apiKey);

      await waitFor(() =>
        expect(apiMocks.setApiKey).toHaveBeenCalledWith(
          providerId,
          "provider-secret"
        )
      );
      expect(apiKey.value).toBe("  provider-secret  ");

      fireEvent.click(visibility);
      expect(apiKey.type).toBe("text");
      fireEvent.click(visibility);
      expect(apiKey.type).toBe("password");
    }
  );

  it.each([
    { provider: "OpenAI", providerId: "openai" },
    { provider: "Groq", providerId: "groq" },
  ] as const)(
    "preserves $provider API Key Enter save and empty-key deletion",
    async ({ provider, providerId }) => {
      const { container } = render(
        <SettingsHarness initialSettings={DEFAULT_SETTINGS} />
      );
      fireEvent.click(screen.getByRole("button", { name: provider }));
      await waitFor(() =>
        expect(apiMocks.getApiKey).toHaveBeenCalledWith(providerId)
      );

      const pane = container.querySelector<HTMLElement>(
        `#pane-${providerId}`
      ) as HTMLElement;
      const apiKey = within(pane).getByPlaceholderText<HTMLInputElement>(
        "Your API Key"
      );

      fireEvent.change(apiKey, { target: { value: "entered-secret" } });
      fireEvent.keyDown(apiKey, { key: "Enter" });
      await waitFor(() =>
        expect(apiMocks.setApiKey).toHaveBeenCalledWith(
          providerId,
          "entered-secret"
        )
      );

      fireEvent.change(apiKey, { target: { value: "" } });
      fireEvent.keyDown(apiKey, { key: "Enter" });
      await waitFor(() =>
        expect(apiMocks.deleteApiKey).toHaveBeenCalledWith(providerId)
      );
      expect(apiKey.getAttribute("placeholder")).toBe("Your API Key");
    }
  );
});

describe("Phase 5 Local provider content", () => {
  it("keeps the Local heading and orders Models before Model Path", () => {
    const { container } = render(
      <SettingsHarness initialSettings={localSettings()} />
    );
    const pane = container.querySelector<HTMLElement>(
      "#pane-local"
    ) as HTMLElement;
    const heading = within(pane).getByRole("heading", {
      level: 2,
      name: "Local Provider",
    });
    const subtitle = within(pane).getByText(
      (_, el) =>
        el?.tagName.toLowerCase() === "p" &&
        (el.textContent?.includes("Manage your Whisper.cpp local models.") ?? false)
    );
    expect(subtitle).not.toBeNull();
    expect(subtitle.textContent).toContain(
      "For additional models, download the files and set the model path."
    );
    const modelsLabel = within(pane).getByText("Models");
    const modelList = pane.querySelector<HTMLElement>(
      "#local-model-list"
    ) as HTMLElement;
    const customRow = localModelOption("Custom Path");
    const modelPathLabel = within(pane).getByText("Model Path");
    const modelPath = customPathInput();
    const folder = within(pane).getByRole("button", {
      name: "Browse model file",
    });

    expect(heading.className).toContain(
      "text-sm font-semibold text-slate-200 tracking-wide"
    );
    expect(subtitle.className).toContain("text-xs text-slate-400 mt-0.5");
    expect(modelList.children).toHaveLength(3);
    expect(modelList.className).toContain("flex flex-col");
    expect(modelList.querySelector(".model-card")).toBeNull();
    expect(
      (subtitle.compareDocumentPosition(modelsLabel) ?? 0) &
        Node.DOCUMENT_POSITION_FOLLOWING
    ).not.toBe(0);
    expect(
      (modelsLabel.compareDocumentPosition(modelList) ?? 0) &
        Node.DOCUMENT_POSITION_FOLLOWING
    ).not.toBe(0);
    expect(
      (modelList.compareDocumentPosition(modelPathLabel) ?? 0) &
        Node.DOCUMENT_POSITION_FOLLOWING
    ).not.toBe(0);
    expect(
      (modelPathLabel.compareDocumentPosition(modelPath) ?? 0) &
        Node.DOCUMENT_POSITION_FOLLOWING
    ).not.toBe(0);
    expect(customRow.textContent?.trim()).toBe("Custom Path");
    expect(customRow.querySelector("input")).toBeNull();
    expect(modelPath.closest("#local-model-list")).toBeNull();
    expect(modelPath.getAttribute("placeholder")).toBe(
      "Custom path to a local model file (.bin)"
    );
    expect(modelPath.parentElement?.className).toContain("border-b");
    expect(modelPath.parentElement?.classList.contains("hud-border")).toBe(false);
    expect(folder.parentElement).toBe(modelPath.parentElement);
    expect(folder.className).toContain("absolute right-2");
  });

  it("exposes no Local recording-duration or VAD tuning control", () => {
    const { container } = render(
      <SettingsHarness initialSettings={localSettings()} />
    );
    const pane = container.querySelector<HTMLElement>("#pane-local") as HTMLElement;

    expect(within(pane).queryByLabelText(/duration|vad/i)).toBeNull();
    expect(
      within(pane).queryByText(/recording duration|vad threshold|minimum silence|maximum speech/i)
    ).toBeNull();
  });

  it("keeps Custom Path as a label-only selection row", async () => {
    render(
      <SettingsHarness
        initialSettings={localSettings("", "", "openai")}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: "Local" }));
    await waitFor(() => expect(apiMocks.listLocalModels).toHaveBeenCalled());
    apiMocks.saveSettings.mockClear();

    const customRow = localModelOption("Custom Path");
    const input = customPathInput();
    const userPath = "Z:\\models\\user-owned.bin";

    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: userPath } });

    expectLocalModelUnselected(customRow);
    expect(apiMocks.saveSettings).not.toHaveBeenCalled();
    expect(customRow.textContent?.trim()).toBe("Custom Path");
    expect(customRow.textContent).not.toContain("user-owned.bin");
    expect(apiMocks.setLocalCustomModelPath).not.toHaveBeenCalled();
    fireEvent.blur(input);
    await waitFor(() =>
      expect(apiMocks.setLocalCustomModelPath).toHaveBeenCalledWith(userPath)
    );

    fireEvent.click(customRow);

    await waitFor(() => expectLocalModelSelected(customRow));
    expect(lastSavedSettings()).toEqual(
      expect.objectContaining({
        active_provider: "openai",
        local_model_kind: "custom",
        local_model_path: userPath,
        local_custom_model_path: userPath,
      })
    );
    expect(input.value).toBe(userPath);
  });

  it("keeps each standard model action at the row's far right", async () => {
    render(<SettingsHarness initialSettings={localSettings()} />);
    const availableRow = localModelOption("Local Environment One");
    const downloadedRow = localModelOption("Local Environment Two");
    const download = await within(availableRow).findByRole("button", {
      name: "Download model",
    });
    const deleteButton = await within(downloadedRow).findByRole("button", {
      name: "Delete model",
    });
    const ready = within(downloadedRow).getByText("Ready");

    expect(availableRow.className).not.toContain("model-card");
    expect(availableRow.className).not.toContain("hud-border");
    expect(availableRow.lastElementChild?.contains(download)).toBe(true);
    expect(downloadedRow.lastElementChild?.contains(deleteButton)).toBe(true);
    expect(downloadedRow.lastElementChild?.contains(ready)).toBe(true);
    expect(
      (ready.compareDocumentPosition(deleteButton) ?? 0) &
        Node.DOCUMENT_POSITION_FOLLOWING
    ).not.toBe(0);

    fireEvent.click(availableRow);
    await waitFor(() => expectLocalModelSelected(availableRow));
  });
});

describe("environment model selection", () => {
  it("uses provider tabs only for navigation", () => {
    render(
      <SettingsHarness
        initialSettings={{ ...DEFAULT_SETTINGS, active_provider: "openai" }}
      />
    );

    const groqTab = screen.getByRole("button", { name: "Groq" });
    const localTab = screen.getByRole("button", { name: "Local" });

    fireEvent.click(groqTab);
    expect(groqTab.className).toContain("active");
    fireEvent.click(localTab);
    expect(localTab.className).toContain("active");
    expect(apiMocks.saveSettings).not.toHaveBeenCalled();
  });

  it("selects an OpenAI model without selecting OpenAI", async () => {
    render(
      <SettingsHarness
        initialSettings={{ ...DEFAULT_SETTINGS, active_provider: "groq" }}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: "OpenAI" }));
    const option = cloudModelOption("OpenAI Environment Two");

    fireEvent.click(option);
    await waitFor(() =>
      expect(apiMocks.selectOpenaiModel).toHaveBeenCalledWith(
        "openai-environment-two"
      )
    );

    expectCloudModelSelected(option);
    expect(lastSavedSettings()).toEqual(
      expect.objectContaining({
        active_provider: "groq",
        openai_model: "openai-environment-two",
        openai_model_kind: "standard",
      })
    );
  });

  it("selects a Groq model without selecting Groq", async () => {
    render(
      <SettingsHarness
        initialSettings={{ ...DEFAULT_SETTINGS, active_provider: "openai" }}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: "Groq" }));
    const option = cloudModelOption("Groq Environment Two");

    fireEvent.click(option);
    await waitFor(() =>
      expect(apiMocks.selectGroqModel).toHaveBeenCalledWith(
        "groq-environment-two"
      )
    );

    expectCloudModelSelected(option);
    expect(lastSavedSettings()).toEqual(
      expect.objectContaining({
        active_provider: "openai",
        groq_model: "groq-environment-two",
        groq_model_kind: "standard",
      })
    );
  });

  it("stores an arbitrary custom cloud ID without preflight", async () => {
    render(
      <SettingsHarness
        initialSettings={{ ...DEFAULT_SETTINGS, active_provider: "groq" }}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: "OpenAI" }));
    const input = screen.getByPlaceholderText("Custom Model ID");

    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "totally-unsupported/id" } });
    expect(apiMocks.setOpenaiCustomModel).not.toHaveBeenCalled();
    fireEvent.blur(input);

    await waitFor(() =>
      expect(lastSavedSettings()).toEqual(
        expect.objectContaining({
          active_provider: "groq",
          openai_model: "totally-unsupported/id",
          openai_model_kind: "custom",
        })
      )
    );
    expect(apiMocks.setApiKey).not.toHaveBeenCalled();
    expect(apiMocks.runDiagnostics).not.toHaveBeenCalled();
  });

  it("does not change custom cloud input when selecting a standard row", async () => {
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
    fireEvent.click(screen.getByRole("button", { name: "OpenAI" }));
    const input = screen.getByPlaceholderText<HTMLInputElement>("Custom Model ID");

    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "user-owned-model-id" } });
    fireEvent.blur(input);
    await waitFor(() =>
      expect(apiMocks.setOpenaiCustomModel).toHaveBeenCalledWith(
        "user-owned-model-id"
      )
    );
    fireEvent.click(cloudModelOption("OpenAI Environment Two"));

    await waitFor(() => expect(input.value).toBe("user-owned-model-id"));
    expect(lastSavedSettings()).toEqual(
      expect.objectContaining({
        openai_model: "openai-environment-two",
        openai_model_kind: "standard",
        openai_custom_model: "user-owned-model-id",
      })
    );
  });

  it.each([
    {
      provider: "OpenAI",
      standardLabel: "OpenAI Environment One",
      modelField: "openai_model",
      kindField: "openai_model_kind",
    },
    {
      provider: "Groq",
      standardLabel: "Groq Environment One",
      modelField: "groq_model",
      kindField: "groq_model_kind",
    },
  ] as const)(
    "keeps $provider Custom selected when its ID is empty",
    async ({ provider, standardLabel, modelField, kindField }) => {
      render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
      fireEvent.click(screen.getByRole("button", { name: provider }));
      const standardOption = cloudModelOption(standardLabel);
      const input =
        screen.getByPlaceholderText<HTMLInputElement>("Custom Model ID");
      const customOption = input.closest(".cloud-model-option") as HTMLElement;

      fireEvent.click(customOption);

      await waitFor(() => expectCloudModelSelected(customOption));
      expectCloudModelUnselected(standardOption);
      expect(document.activeElement).toBe(input);
      expect(lastSavedSettings()).toEqual(
        expect.objectContaining({
          [kindField]: "custom",
          [modelField]: "",
        })
      );

      fireEvent.blur(input);
      await waitFor(() => expectCloudModelSelected(customOption));
    }
  );

  it.each([
    {
      provider: "OpenAI",
      standardLabel: "OpenAI Environment One",
      standardId: "openai-environment-one",
      modelField: "openai_model",
      kindField: "openai_model_kind",
    },
    {
      provider: "Groq",
      standardLabel: "Groq Environment One",
      standardId: "groq-environment-one",
      modelField: "groq_model",
      kindField: "groq_model_kind",
    },
  ] as const)(
    "keeps $provider Custom selected when its ID equals a standard ID",
    async ({
      provider,
      standardLabel,
      standardId,
      modelField,
      kindField,
    }) => {
      render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
      fireEvent.click(screen.getByRole("button", { name: provider }));
      const standardOption = cloudModelOption(standardLabel);
      const input =
        screen.getByPlaceholderText<HTMLInputElement>("Custom Model ID");
      const customOption = input.closest(".cloud-model-option") as HTMLElement;

      fireEvent.focus(input);
      fireEvent.change(input, { target: { value: standardId } });
      fireEvent.blur(input);

      await waitFor(() => expectCloudModelSelected(customOption));
      expectCloudModelUnselected(standardOption);
      expect(lastSavedSettings()).toEqual(
        expect.objectContaining({
          [kindField]: "custom",
          [modelField]: standardId,
        })
      );

      fireEvent.change(input, { target: { value: "" } });
      fireEvent.blur(input);
      await waitFor(() => expectCloudModelSelected(customOption));
      expectCloudModelUnselected(standardOption);
      expect(lastSavedSettings()).toEqual(
        expect.objectContaining({
          [kindField]: "custom",
          [modelField]: "",
        })
      );
    }
  );

  it.each([
    {
      provider: "OpenAI",
      modelField: "openai_model",
      kindField: "openai_model_kind",
      customField: "openai_custom_model",
    },
    {
      provider: "Groq",
      modelField: "groq_model",
      kindField: "groq_model_kind",
      customField: "groq_custom_model",
    },
  ] as const)(
    "hydrates persisted $provider Custom state without rewriting it",
    async ({ provider, modelField, kindField, customField }) => {
      const initialSettings: AppSettings = {
        ...DEFAULT_SETTINGS,
        [modelField]: "persisted-custom-id",
        [kindField]: "custom",
        [customField]: "persisted-custom-id",
      };
      render(<SettingsHarness initialSettings={initialSettings} />);
      fireEvent.click(screen.getByRole("button", { name: provider }));
      const input =
        screen.getByPlaceholderText<HTMLInputElement>("Custom Model ID");
      const customOption = input.closest(".cloud-model-option") as HTMLElement;

      await waitFor(() => expectCloudModelSelected(customOption));
      expect(input.value).toBe("persisted-custom-id");
      expect(apiMocks.setOpenaiCustomModel).not.toHaveBeenCalled();
      expect(apiMocks.setGroqCustomModel).not.toHaveBeenCalled();
    }
  );

  it("sends the exact Local environment download path", async () => {
    render(<SettingsHarness initialSettings={localSettings()} />);
    const card = localModelOption("Local Environment One");
    const download = within(card).getByRole("button", {
      name: "Download model",
    });

    fireEvent.click(download);

    await waitFor(() =>
      expect(apiMocks.downloadModel).toHaveBeenCalledWith(
        "https://models.example.test/local-one.bin"
      )
    );
  });
});

describe("Local model download lifecycle", () => {
  it("renders exact known progress without visible progress text", async () => {
    render(<SettingsHarness initialSettings={localSettings()} />);
    const card = localModelOption("Local Environment One");
    fireEvent.click(
      within(card).getByRole("button", { name: "Download model" })
    );

    await waitFor(() =>
      expect(apiMocks.downloadModel).toHaveBeenCalledWith(
        modelMapMocks.local[0].downloadPath
      )
    );
    expectLocalModelUnselected(card);
    const cancel = within(card).getByRole("button", {
      name: "Cancel model download",
    });
    expect(card.lastElementChild?.contains(cancel)).toBe(true);
    let progress = within(card).getByRole("progressbar");
    expect(
      (progress.compareDocumentPosition(cancel) ?? 0) &
        Node.DOCUMENT_POSITION_FOLLOWING
    ).not.toBe(0);
    expect(progress.getAttribute("aria-valuenow")).toBeNull();
    expect(progress.firstElementChild?.className).toContain("w-0");

    act(() => {
      tauriEventMocks.emit("model:download:progress", {
        download_path: modelMapMocks.local[1].downloadPath,
        downloaded_bytes: 90,
        total_bytes: 100,
      });
    });
    progress = within(card).getByRole("progressbar");
    expect(progress.getAttribute("aria-valuenow")).toBeNull();

    act(() => {
      tauriEventMocks.emit("model:download:progress", {
        download_path: modelMapMocks.local[0].downloadPath,
        downloaded_bytes: 25,
        total_bytes: 100,
      });
    });
    progress = within(card).getByRole("progressbar");
    expect(progress.getAttribute("aria-valuemin")).toBe("0");
    expect(progress.getAttribute("aria-valuenow")).toBe("25");
    expect(progress.getAttribute("aria-valuemax")).toBe("100");
    expect(
      (progress.firstElementChild as HTMLElement).style.width
    ).toBe("25%");
    expect(within(card).queryByText(/25|%/)).toBeNull();
  });

  it("renders unknown-size progress without inventing a value", async () => {
    render(<SettingsHarness initialSettings={localSettings()} />);
    const card = localModelOption("Local Environment One");
    fireEvent.click(
      within(card).getByRole("button", { name: "Download model" })
    );
    await waitFor(() =>
      expect(
        within(card).getByRole("button", {
          name: "Cancel model download",
        })
      ).toBeTruthy()
    );

    act(() => {
      tauriEventMocks.emit("model:download:progress", {
        download_path: modelMapMocks.local[0].downloadPath,
        downloaded_bytes: 512,
        total_bytes: null,
      });
    });

    const progress = within(card).getByRole("progressbar");
    expect(progress.getAttribute("aria-valuenow")).toBeNull();
    expect(progress.getAttribute("aria-valuemax")).toBeNull();
    expect(progress.textContent).toBe("");
    expect(progress.firstElementChild?.className).toContain("w-1/3");
    expect(progress.firstElementChild?.className).toContain("animate-pulse");
    expect(progress.firstElementChild?.className).not.toContain("w-0");
  });

  it("ignores malformed download lifecycle payloads", async () => {
    render(<SettingsHarness initialSettings={localSettings()} />);
    const card = localModelOption("Local Environment One");
    fireEvent.click(
      within(card).getByRole("button", { name: "Download model" })
    );
    await within(card).findByRole("button", {
      name: "Cancel model download",
    });

    act(() => {
      tauriEventMocks.emit("model:download:progress", null);
      tauriEventMocks.emit("model:download:complete", { path: "missing-identity" });
      tauriEventMocks.emit("model:download:cancelled", 42);
      tauriEventMocks.emit("model:download:error", { code: "model_download" });
    });

    expect(
      within(card).getByRole("button", { name: "Cancel model download" })
    ).toBeTruthy();
    expect(screen.queryByText(USER_ERROR_MESSAGES.model_download)).toBeNull();
  });

  it("cancels by exact path and treats cancellation as non-failure", async () => {
    render(<SettingsHarness initialSettings={localSettings()} />);
    const card = localModelOption("Local Environment One");
    fireEvent.click(
      within(card).getByRole("button", { name: "Download model" })
    );
    const cancel = await within(card).findByRole("button", {
      name: "Cancel model download",
    });

    fireEvent.click(cancel);
    await waitFor(() =>
      expect(apiMocks.cancelDownload).toHaveBeenCalledWith(
        modelMapMocks.local[0].downloadPath
      )
    );
    expect((cancel as HTMLButtonElement).disabled).toBe(true);

    act(() => {
      tauriEventMocks.emit("model:download:cancelled", {
        download_path: modelMapMocks.local[0].downloadPath,
      });
    });

    expect(
      within(card).getByRole("button", { name: "Download model" })
    ).toBeTruthy();
    expect(within(card).queryByText("Ready")).toBeNull();
    expect(screen.queryByText(/cancelled/i)).toBeNull();
  });

  it("shows Ready and Check before transitioning to Trash", async () => {
    render(<SettingsHarness initialSettings={localSettings()} />);
    const card = localModelOption("Local Environment One");
    fireEvent.click(
      within(card).getByRole("button", { name: "Download model" })
    );
    await waitFor(() =>
      expect(
        within(card).getByRole("button", {
          name: "Cancel model download",
        })
      ).toBeTruthy()
    );
    apiMocks.listLocalModels.mockResolvedValue([
      { ...localInfos[0], downloaded: true, size_bytes: 1_000 },
      localInfos[1],
    ]);
    vi.useFakeTimers();

    act(() => {
      tauriEventMocks.emit("model:download:complete", {
        download_path: modelMapMocks.local[0].downloadPath,
        path: localInfos[0].path,
      });
    });

    expect(within(card).queryByRole("progressbar")).toBeNull();
    const ready = within(card).getByText("Ready");
    const check = within(card).getByRole("img", {
      name: "Download complete",
    });
    expect(card.lastElementChild?.contains(check)).toBe(true);
    expect(
      (ready.compareDocumentPosition(check) ?? 0) &
        Node.DOCUMENT_POSITION_FOLLOWING
    ).not.toBe(0);
    expect(
      within(card).queryByRole("button", { name: "Delete model" })
    ).toBeNull();

    act(() => vi.advanceTimersByTime(1_000));

    expect(
      within(card).getByRole("button", { name: "Delete model" })
    ).toBeTruthy();
    expect(within(card).getByText("Ready")).toBeTruthy();
  });

  it("restores Download and reports a genuine download failure", async () => {
    render(<SettingsHarness initialSettings={localSettings()} />);
    const card = localModelOption("Local Environment One");
    fireEvent.click(
      within(card).getByRole("button", { name: "Download model" })
    );
    await waitFor(() =>
      expect(
        within(card).getByRole("button", {
          name: "Cancel model download",
        })
      ).toBeTruthy()
    );

    act(() => {
      tauriEventMocks.emit("model:download:error", {
        download_path: modelMapMocks.local[0].downloadPath,
        code: "model_download",
      });
    });

    expect(
      within(card).getByRole("button", { name: "Download model" })
    ).toBeTruthy();
    expect(screen.getByText("Unable to download the model")).toBeTruthy();
    expect(within(card).queryByText("Ready")).toBeNull();
  });

  it("shows Ready and Trash immediately for an existing download", async () => {
    render(<SettingsHarness initialSettings={localSettings()} />);
    const card = localModelOption("Local Environment Two");

    const deleteButton = await within(card).findByRole("button", {
      name: "Delete model",
    });
    const ready = within(card).getByText("Ready");
    expect(card.lastElementChild?.contains(deleteButton)).toBe(true);
    expect(
      (ready.compareDocumentPosition(deleteButton) ?? 0) &
        Node.DOCUMENT_POSITION_FOLLOWING
    ).not.toBe(0);
    expect(
      within(card).queryByRole("img", { name: "Download complete" })
    ).toBeNull();
  });
});

describe("Local model deletion", () => {
  it("dismisses the themed confirmation with Cancel or backdrop", async () => {
    render(<SettingsHarness initialSettings={localSettings()} />);
    const card = localModelOption("Local Environment Two");
    const trash = await within(card).findByRole("button", {
      name: "Delete model",
    });

    fireEvent.click(trash);
    let dialog = screen.getByRole("dialog", {
      name: "Are you sure you want to delete the model?",
    });
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Cancel" })
    );
    expectLocalModelUnselected(card);

    fireEvent.click(dialog);
    expect(screen.getByRole("dialog")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(apiMocks.deleteModel).not.toHaveBeenCalled();

    fireEvent.click(trash);
    dialog = screen.getByRole("dialog");
    fireEvent.click(dialog.parentElement as HTMLElement);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(apiMocks.deleteModel).not.toHaveBeenCalled();
  });

  it("confirms exact-path deletion and synchronizes selected settings", async () => {
    const selectedSettings = localSettings(
      localInfos[1].path,
      "standard",
      "local"
    );
    apiMocks.deleteModel.mockResolvedValue(localSettings());
    render(<SettingsHarness initialSettings={selectedSettings} />);
    const card = localModelOption("Local Environment Two");
    const trash = await within(card).findByRole("button", {
      name: "Delete model",
    });
    fireEvent.click(trash);
    apiMocks.listLocalModels.mockResolvedValue([
      localInfos[0],
      { ...localInfos[1], downloaded: false, size_bytes: null },
    ]);

    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() =>
      expect(apiMocks.deleteModel).toHaveBeenCalledWith(
        modelMapMocks.local[1].downloadPath
      )
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expectLocalModelUnselected(card);
    expect(
      within(card).getByRole("button", { name: "Download model" })
    ).toBeTruthy();
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Start" }).disabled
    ).toBe(true);
  });
});

describe("Local model selection", () => {
  it("does not display a standard model path in Custom Path", () => {
    render(
      <SettingsHarness
        initialSettings={localSettings(
          localInfos[0].path,
          "standard",
          "openai"
        )}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: "Local" }));

    expect(customPathInput().value).toBe("");
  });

  it("hydrates Custom Path only from its isolated persisted field", () => {
    const persistedPath = "persisted-user-path-that-must-be-preserved";
    render(
      <SettingsHarness
        initialSettings={localSettings(persistedPath, "custom")}
      />
    );

    expect(customPathInput().value).toBe(persistedPath);
    expectLocalModelSelected(localModelOption("Custom Path"));
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Start" }).disabled
    ).toBe(false);
  });

  it("preserves user text when either standard model is selected", async () => {
    render(<SettingsHarness initialSettings={localSettings()} />);
    const input = customPathInput();
    const userPath = "Z:\\missing-models\\user-selected.bin";

    fireEvent.change(input, { target: { value: userPath } });
    fireEvent.blur(input);
    await waitFor(() =>
      expect(apiMocks.setLocalCustomModelPath).toHaveBeenCalledWith(userPath)
    );
    fireEvent.click(localModelOption("Local Environment One"));
    await waitFor(() =>
      expect(apiMocks.selectLocalModel).toHaveBeenCalled()
    );
    expect(input.value).toBe(userPath);
    expect(lastSavedSettings()).toEqual(
      expect.objectContaining({ local_custom_model_path: userPath })
    );

    fireEvent.click(localModelOption("Local Environment Two"));
    expect(input.value).toBe(userPath);
  });

  it("configures Local only for the selected downloaded standard model", async () => {
    render(<SettingsHarness initialSettings={localSettings()} />);
    const start = screen.getByRole<HTMLButtonElement>("button", { name: "Start" });

    await waitFor(() =>
      expect(
        screen.getAllByRole("button", { name: "Download model" })
      ).toHaveLength(1)
    );

    fireEvent.click(localModelOption("Local Environment One"));
    expect(start.disabled).toBe(true);

    fireEvent.click(localModelOption("Local Environment Two"));
    await waitFor(() => expect(start.disabled).toBe(false));
  });

  it("selects a Local model without selecting Local", async () => {
    render(
      <SettingsHarness
        initialSettings={localSettings("", "", "openai")}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: "Local" }));

    fireEvent.click(localModelOption("Local Environment Two"));

    await waitFor(() =>
      expect(lastSavedSettings()).toEqual(
        expect.objectContaining({
          active_provider: "openai",
          local_model_kind: "standard",
          local_model_path: localInfos[1].path,
        })
      )
    );
  });

  it("configures Local for any non-empty Custom Path without validation", async () => {
    render(<SettingsHarness initialSettings={localSettings()} />);
    const start = screen.getByRole<HTMLButtonElement>("button", { name: "Start" });

    fireEvent.click(localModelOption("Custom Path"));
    expect(start.disabled).toBe(true);
    fireEvent.change(customPathInput(), {
      target: { value: "not-a-file-and-not-a-bin-extension" },
    });

    await waitFor(() => expect(start.disabled).toBe(false));
    expect(apiMocks.setLocalCustomModelPath).not.toHaveBeenCalled();
    fireEvent.blur(customPathInput());
    await waitFor(() =>
      expect(apiMocks.setLocalCustomModelPath).toHaveBeenCalledWith(
        "not-a-file-and-not-a-bin-extension"
      )
    );
    expect(lastSavedSettings()).toEqual(
      expect.objectContaining({
        local_model_kind: "custom",
        local_model_path: "not-a-file-and-not-a-bin-extension",
        local_custom_model_path: "not-a-file-and-not-a-bin-extension",
      })
    );
    expect(apiMocks.runDiagnostics).not.toHaveBeenCalled();
  });

  it("selects empty Custom Path without selecting Local", async () => {
    render(
      <SettingsHarness
        initialSettings={localSettings("", "", "openai")}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: "Local" }));
    const input = customPathInput();
    const card = localModelOption("Custom Path");

    fireEvent.click(card);

    await waitFor(() => expectLocalModelSelected(card));
    expect(input.value).toBe("");
    expect(lastSavedSettings()).toEqual(
      expect.objectContaining({
        active_provider: "openai",
        local_model_kind: "custom",
        local_model_path: "",
      })
    );
  });

  it("changes only from user input or the folder button", async () => {
    const selectedPath = "D:\\models\\my-custom-model.bin";
    dialogMocks.open.mockResolvedValueOnce(selectedPath);
    render(<SettingsHarness initialSettings={localSettings()} />);

    fireEvent.click(screen.getByRole("button", { name: "Browse model file" }));

    await waitFor(() => expect(customPathInput().value).toBe(selectedPath));
    expect(dialogMocks.open).toHaveBeenCalledWith({
      multiple: false,
      directory: false,
      filters: [{ name: "Whisper Model (.bin)", extensions: ["bin"] }],
    });
  });
});

describe("Settings content", () => {
  it("keeps the title, subtitle, and shortcut selector presentation", () => {
    const { container } = render(
      <SettingsHarness initialSettings={DEFAULT_SETTINGS} />
    );
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));

    const pane = container.querySelector<HTMLElement>("#pane-settings");
    expect(pane).not.toBeNull();
    expect(
      within(pane as HTMLElement).getByRole("heading", {
        name: "Global",
      })
    ).toBeTruthy();
    const subtitle = within(pane as HTMLElement).getByText(
      (_, el) =>
        el?.tagName.toLowerCase() === "p" &&
        (el.textContent?.includes("Select your active provider.") ?? false)
    );
    expect(subtitle).not.toBeNull();
    expect(subtitle.textContent).toContain(
      "Use the default shortcut, or set up your own, to start a transcription."
    );
    expect(within(pane as HTMLElement).getByText("Active Provider")).toBeTruthy();
    expect(within(pane as HTMLElement).getByText("Shortcut")).toBeTruthy();

    const input = within(pane as HTMLElement).getByRole<HTMLInputElement>(
      "textbox"
    );
    const underline = input.parentElement;
    expect(input.value).toBe("Ctrl+Alt+Space");
    expect(input.classList.contains("w-full")).toBe(true);
    expect(input.classList.contains("text-center")).toBe(true);
    expect(input.classList.contains("bg-transparent")).toBe(true);
    expect(input.classList.contains("rounded-lg")).toBe(false);
    expect(input.classList.contains("hud-border")).toBe(false);
    expect(underline?.classList.contains("w-full")).toBe(true);
    expect(underline?.classList.contains("border-b")).toBe(true);
    expect(underline?.classList.contains("border-hud-border")).toBe(true);
  });

  it("lists every provider in order and persists an unconfigured selection", async () => {
    const onSaved = vi.fn();
    render(
      <SettingsHarness
        initialSettings={DEFAULT_SETTINGS}
        onSaved={onSaved}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));

    const group = screen.getByRole("radiogroup", {
      name: "Active Provider",
    });
    const options = within(group).getAllByRole("radio");
    expect(options.map((option) => option.textContent)).toEqual([
      "Local",
      "OpenAI",
      "Groq",
    ]);
    expect(options[0].getAttribute("aria-checked")).toBe("true");

    fireEvent.click(within(group).getByRole("radio", { name: "OpenAI" }));

    await waitFor(() =>
      expect(apiMocks.selectActiveProvider).toHaveBeenCalledWith("openai")
    );
    await waitFor(() =>
      expect(
        within(group)
          .getByRole("radio", { name: "OpenAI" })
          .getAttribute("aria-checked")
      ).toBe("true")
    );
    expect(onSaved).toHaveBeenCalledWith(
      expect.objectContaining({ active_provider: "openai" })
    );
    expect(apiMocks.setApiKey).not.toHaveBeenCalled();
    expect(apiMocks.selectOpenaiModel).not.toHaveBeenCalled();
  });

  it("mirrors a persisted tray selection and applies reconciled parent navigation", async () => {
    const onSaved = vi.fn();
    const initialSettings = { ...DEFAULT_SETTINGS, active_provider: "openai" as const };
    const view = render(
      <SettingsHarness
        initialSettings={initialSettings}
        onSaved={onSaved}
      />
    );
    expect(view.container.querySelector("#pane-openai")).not.toBeNull();

    view.rerender(
      <SettingsHarness
        initialSettings={initialSettings}
        onSaved={onSaved}
        navigationTab="settings"
      />
    );
    expect(view.container.querySelector("#pane-settings")).not.toBeNull();

    act(() =>
      tauriEventMocks.emit("settings:active-provider-changed", "groq")
    );
    await waitFor(() =>
      expect(
        screen
          .getByRole("radio", { name: "Groq" })
          .getAttribute("aria-checked")
      ).toBe("true")
    );
    expect(onSaved).toHaveBeenCalledWith(
      expect.objectContaining({ active_provider: "groq" })
    );
    expect(apiMocks.selectActiveProvider).not.toHaveBeenCalled();
  });

  it("ignores malformed committed-provider events", () => {
    const onSaved = vi.fn();
    render(
      <SettingsHarness
        initialSettings={DEFAULT_SETTINGS}
        onSaved={onSaved}
        navigationTab="settings"
      />
    );

    act(() => {
      tauriEventMocks.emit("settings:active-provider-changed", "not-a-provider");
    });

    expect(
      screen.getByRole("radio", { name: "Local" }).getAttribute("aria-checked")
    ).toBe("true");
    expect(onSaved).not.toHaveBeenCalled();
  });

  it("keeps the existing conflict advisory in the shared inline warning slot", async () => {
    apiMocks.isHotkeyRegistered.mockResolvedValueOnce(true);
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));

    const warning = await screen.findByText(
      "⚠ Combination registered by another app."
    );
    expect(warning.className).toBe(
      "text-xs text-amber-400 text-center leading-tight"
    );
  });

  it("ignores a stale conflict result from the previously displayed shortcut", async () => {
    vi.useFakeTimers();
    let resolvePrevious!: (registered: boolean) => void;
    let resolveCurrent!: (registered: boolean) => void;
    apiMocks.isHotkeyRegistered
      .mockImplementationOnce(
        () =>
          new Promise<boolean>((resolve) => {
            resolvePrevious = resolve;
          })
      )
      .mockImplementationOnce(
        () =>
          new Promise<boolean>((resolve) => {
            resolveCurrent = resolve;
          })
      );

    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    const input = screen.getByRole<HTMLInputElement>("textbox");

    act(() => {
      vi.advanceTimersByTime(250);
    });
    expect(apiMocks.isHotkeyRegistered).toHaveBeenCalledWith(
      DEFAULT_SETTINGS.hotkey
    );

    await act(async () => {
      fireEvent.focus(input);
      fireEvent.keyDown(input, {
        key: "k",
        ctrlKey: true,
        shiftKey: true,
      });
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(input.value).toBe("Ctrl+Shift+K");

    act(() => {
      vi.advanceTimersByTime(250);
    });
    expect(apiMocks.isHotkeyRegistered).toHaveBeenLastCalledWith(
      "Ctrl+Shift+K"
    );

    await act(async () => {
      resolveCurrent(false);
      await Promise.resolve();
      resolvePrevious(true);
      await Promise.resolve();
    });

    expect(
      screen.queryByText("⚠ Combination registered by another app.")
    ).toBeNull();
  });

  it("keeps the existing single-key advisory in the shared inline warning slot", () => {
    render(
      <SettingsHarness
        initialSettings={{ ...DEFAULT_SETTINGS, hotkey: "F9" }}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));

    const warning = screen.getByText(
      "⚠ Key without modifiers may interfere with typing."
    );
    expect(warning.className).toBe(
      "text-xs text-amber-400 text-center leading-tight"
    );
  });

  it("ignores a stale mount-time hotkey availability result after a newer save refresh", async () => {
    let resolveMountStatus!: (active: boolean) => void;
    apiMocks.isHotkeyActive
      .mockImplementationOnce(
        () =>
          new Promise<boolean>((resolve) => {
            resolveMountStatus = resolve;
          })
      )
      .mockResolvedValueOnce(true);

    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    const input = screen.getByRole<HTMLInputElement>("textbox");

    await act(async () => {
      fireEvent.focus(input);
      fireEvent.keyDown(input, {
        key: "k",
        ctrlKey: true,
        shiftKey: true,
      });
      await Promise.resolve();
      await Promise.resolve();
    });
    await waitFor(() => expect(apiMocks.isHotkeyActive).toHaveBeenCalledTimes(2));
    expect(input.value).toBe("Ctrl+Shift+K");

    await act(async () => {
      resolveMountStatus(false);
      await Promise.resolve();
    });

    expect(
      screen.queryByText("⚠ Global shortcuts are unavailable on this system.")
    ).toBeNull();
  });

  it("shows startup global-shortcut unavailability in the shared inline warning slot", async () => {
    apiMocks.isHotkeyActive.mockResolvedValueOnce(false);
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));

    const warning = await screen.findByText(
      "⚠ Global shortcuts are unavailable on this system."
    );
    expect(warning.className).toBe(
      "text-xs text-amber-400 text-center leading-tight"
    );
  });

  it("keeps the saved shortcut and shows a transient shared warning when replacement fails", async () => {
    vi.useFakeTimers();
    apiMocks.setHotkey.mockRejectedValueOnce("invalid_shortcut");
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    const input = screen.getByRole<HTMLInputElement>("textbox");

    await act(async () => {
      fireEvent.focus(input);
      fireEvent.keyDown(input, {
        key: "k",
        ctrlKey: true,
        shiftKey: true,
      });
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(input.value).toBe(DEFAULT_SETTINGS.hotkey);
    const warning = screen.getByText("⚠ Your shortcut could not be saved.");
    expect(warning.className).toBe(
      "text-xs text-amber-400 text-center leading-tight"
    );
    expect(screen.queryByText(USER_ERROR_MESSAGES.invalid_shortcut)).toBeNull();

    act(() => {
      vi.advanceTimersByTime(1500);
    });
    expect(screen.queryByText("⚠ Your shortcut could not be saved.")).toBeNull();
  });

  it("preserves hotkey capture and persistence behavior", async () => {
    const onSaved = vi.fn();
    render(
      <SettingsHarness
        initialSettings={DEFAULT_SETTINGS}
        onSaved={onSaved}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    const input = screen.getByRole<HTMLInputElement>("textbox");

    fireEvent.focus(input);
    expect(input.value).toBe("Press key...");
    fireEvent.keyDown(input, {
      key: "k",
      ctrlKey: true,
      shiftKey: true,
    });

    await waitFor(() => expect(input.value).toBe("Ctrl+Shift+K"));
    expect(onSaved).toHaveBeenCalledWith(
      expect.objectContaining({ hotkey: "Ctrl+Shift+K" })
    );
    expect(apiMocks.setHotkey).toHaveBeenCalledWith("Ctrl+Shift+K");
    expect(apiMocks.saveSettings).not.toHaveBeenCalled();
  });

  it("forwards focused Settings key transitions to the backend adapter", async () => {
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

    fireEvent.keyDown(window, {
      key: "a",
      shiftKey: true,
      metaKey: true,
      repeat: false,
    });
    fireEvent.keyUp(window, {
      key: "a",
      shiftKey: true,
      metaKey: true,
    });

    await waitFor(() =>
      expect(apiMocks.sendFocusedHotkeyKeyEvent).toHaveBeenCalledTimes(2)
    );
    expect(apiMocks.sendFocusedHotkeyKeyEvent).toHaveBeenNthCalledWith(1, {
      key: "a",
      ctrlKey: false,
      altKey: false,
      shiftKey: true,
      metaKey: true,
      pressed: true,
      repeat: false,
    });
    expect(apiMocks.sendFocusedHotkeyKeyEvent).toHaveBeenNthCalledWith(2, {
      key: "a",
      ctrlKey: false,
      altKey: false,
      shiftKey: true,
      metaKey: true,
      pressed: false,
      repeat: false,
    });
  });

  it("suppresses runtime dispatch while the shortcut recorder captures input", async () => {
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    const input = screen.getByRole<HTMLInputElement>("textbox");

    fireEvent.focusIn(input);
    await waitFor(() =>
      expect(apiMocks.setHotkeyCaptureActive).toHaveBeenCalledWith(true)
    );
    fireEvent.keyDown(input, {
      key: "k",
      ctrlKey: true,
      shiftKey: true,
    });

    expect(apiMocks.sendFocusedHotkeyKeyEvent).not.toHaveBeenCalled();
    fireEvent.focusOut(input);
    await waitFor(() =>
      expect(apiMocks.setHotkeyCaptureActive).toHaveBeenCalledWith(false)
    );
  });

  it("resets focused input state when the Settings window loses focus", async () => {
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);

    fireEvent.blur(window);

    await waitFor(() =>
      expect(apiMocks.resetFocusedHotkeyInput).toHaveBeenCalledTimes(1)
    );
  });
});

describe("field-scoped persistence and completion", () => {
  it("hides Start after onboarding while retaining readiness information", () => {
    render(
      <SettingsHarness
        initialSettings={{
          ...DEFAULT_SETTINGS,
          onboarding_complete: true,
        }}
      />
    );

    expect(screen.queryByRole("button", { name: "Start" })).toBeNull();
    expect(screen.getByText("Configure at least one provider.")).toBeTruthy();
  });

  it("does not rewrite an unchanged loaded API key when closing", async () => {
    const onFinish = vi.fn().mockResolvedValue(undefined);
    apiMocks.getApiKey.mockImplementation((provider: string) =>
      Promise.resolve(provider === "openai" ? "stored-openai-key" : "")
    );
    render(
      <SettingsHarness
        initialSettings={{
          ...DEFAULT_SETTINGS,
          active_provider: "openai",
          onboarding_complete: true,
        }}
        onFinish={onFinish}
      />
    );
    const key = await screen.findByPlaceholderText<HTMLInputElement>(
      "Your API Key"
    );
    await waitFor(() => expect(key.value).toBe("stored-openai-key"));

    fireEvent.click(screen.getByRole("button", { name: "Close" }));

    await waitFor(() => expect(windowMocks.hide).toHaveBeenCalledTimes(1));
    expect(apiMocks.setApiKey).not.toHaveBeenCalled();
    expect(apiMocks.deleteApiKey).not.toHaveBeenCalled();
    expect(onFinish).not.toHaveBeenCalled();
  });

  it("flushes one dirty API key before completion and hide", async () => {
    const onFinish = vi.fn().mockResolvedValue(undefined);
    render(
      <SettingsHarness
        initialSettings={{ ...DEFAULT_SETTINGS, active_provider: "openai" }}
        onFinish={onFinish}
      />
    );
    const key = await screen.findByPlaceholderText<HTMLInputElement>(
      "Your API Key"
    );
    fireEvent.change(key, { target: { value: "  replacement-key  " } });

    fireEvent.click(screen.getByRole("button", { name: "Close" }));

    await waitFor(() =>
      expect(apiMocks.setApiKey).toHaveBeenCalledWith(
        "openai",
        "replacement-key"
      )
    );
    expect(apiMocks.setApiKey).toHaveBeenCalledTimes(1);
    expect(onFinish).toHaveBeenCalledTimes(1);
    expect(windowMocks.hide).toHaveBeenCalledTimes(1);
    expect(apiMocks.setApiKey.mock.invocationCallOrder[0]).toBeLessThan(
      onFinish.mock.invocationCallOrder[0]
    );
    expect(onFinish.mock.invocationCallOrder[0]).toBeLessThan(
      windowMocks.hide.mock.invocationCallOrder[0]
    );
    expect(apiMocks.saveSettings).not.toHaveBeenCalled();
  });

  it("deletes a loaded API key only after the cleared draft is persisted", async () => {
    apiMocks.getApiKey.mockImplementation((provider: string) =>
      Promise.resolve(provider === "openai" ? "stored-openai-key" : "")
    );
    render(
      <SettingsHarness
        initialSettings={{
          ...DEFAULT_SETTINGS,
          active_provider: "openai",
          onboarding_complete: true,
        }}
      />
    );
    const key = await screen.findByPlaceholderText<HTMLInputElement>(
      "Your API Key"
    );
    await waitFor(() => expect(key.value).toBe("stored-openai-key"));

    fireEvent.change(key, { target: { value: "" } });
    expect(apiMocks.deleteApiKey).not.toHaveBeenCalled();
    fireEvent.blur(key);

    await waitFor(() =>
      expect(apiMocks.deleteApiKey).toHaveBeenCalledWith("openai")
    );
    expect(apiMocks.setApiKey).not.toHaveBeenCalled();
  });

  it("persists a Custom Model ID once on blur without touching Groq", async () => {
    render(
      <SettingsHarness
        initialSettings={{
          ...DEFAULT_SETTINGS,
          active_provider: "openai",
          openai_custom_model: "old-openai-custom",
          groq_custom_model: "preserve-groq-custom",
        }}
      />
    );
    const input = screen.getByPlaceholderText<HTMLInputElement>(
      "Custom Model ID"
    );
    fireEvent.focus(input);
    await waitFor(() =>
      expect(apiMocks.selectOpenaiCustomModel).toHaveBeenCalledTimes(1)
    );
    apiMocks.setOpenaiCustomModel.mockClear();

    fireEvent.change(input, { target: { value: "draft-one" } });
    fireEvent.change(input, { target: { value: "final-openai-custom" } });
    expect(apiMocks.setOpenaiCustomModel).not.toHaveBeenCalled();
    fireEvent.blur(input);

    await waitFor(() =>
      expect(apiMocks.setOpenaiCustomModel).toHaveBeenCalledWith(
        "final-openai-custom"
      )
    );
    expect(apiMocks.setOpenaiCustomModel).toHaveBeenCalledTimes(1);
    expect(apiMocks.setGroqCustomModel).not.toHaveBeenCalled();
    expect(lastSavedSettings()).toEqual(
      expect.objectContaining({
        openai_model: "final-openai-custom",
        openai_custom_model: "final-openai-custom",
        groq_custom_model: "preserve-groq-custom",
      })
    );
    expect(apiMocks.saveSettings).not.toHaveBeenCalled();
  });

  it("keeps a folder-picked Local path draft until shared Close flushes it", async () => {
    const selectedPath = "D:\\models\\folder-picked.bin";
    dialogMocks.open.mockResolvedValueOnce(selectedPath);
    render(
      <SettingsHarness
        initialSettings={{
          ...localSettings(),
          onboarding_complete: true,
        }}
      />
    );

    fireEvent.click(screen.getByRole("button", { name: "Browse model file" }));
    await waitFor(() => expect(customPathInput().value).toBe(selectedPath));
    expect(apiMocks.setLocalCustomModelPath).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Close" }));

    await waitFor(() =>
      expect(apiMocks.setLocalCustomModelPath).toHaveBeenCalledWith(selectedPath)
    );
    expect(windowMocks.hide).toHaveBeenCalledTimes(1);
    expect(lastSavedSettings()).toEqual(
      expect.objectContaining({
        local_model_path: "",
        local_custom_model_path: selectedPath,
      })
    );
  });

  it("preserves and persists a newer API key edit made while the previous save is in flight", async () => {
    let resolveFirstSave!: () => void;
    apiMocks.setApiKey
      .mockImplementationOnce(
        () =>
          new Promise<void>((resolve) => {
            resolveFirstSave = resolve;
          })
      )
      .mockResolvedValue(undefined);

    render(
      <SettingsHarness
        initialSettings={{ ...DEFAULT_SETTINGS, active_provider: "openai" }}
      />
    );
    const key = await screen.findByPlaceholderText<HTMLInputElement>("Your API Key");

    fireEvent.change(key, { target: { value: "first-key" } });
    fireEvent.blur(key);
    await waitFor(() =>
      expect(apiMocks.setApiKey).toHaveBeenCalledWith("openai", "first-key")
    );

    fireEvent.change(key, { target: { value: "second-key" } });
    await act(async () => {
      resolveFirstSave();
      await Promise.resolve();
    });

    await waitFor(() =>
      expect(apiMocks.setApiKey).toHaveBeenLastCalledWith("openai", "second-key")
    );
    expect(apiMocks.setApiKey).toHaveBeenCalledTimes(2);
    expect(key.value).toBe("second-key");
  });

  it("does not let delayed initial API key hydration overwrite a newer user edit", async () => {
    let resolveOpenaiKey!: (value: string) => void;
    apiMocks.getApiKey.mockImplementation((provider: string) =>
      provider === "openai"
        ? new Promise<string>((resolve) => {
            resolveOpenaiKey = resolve;
          })
        : Promise.resolve("")
    );

    render(
      <SettingsHarness
        initialSettings={{ ...DEFAULT_SETTINGS, active_provider: "openai" }}
      />
    );
    const key = await screen.findByPlaceholderText<HTMLInputElement>("Your API Key");
    fireEvent.change(key, { target: { value: "new-user-key" } });

    await act(async () => {
      resolveOpenaiKey("older-stored-key");
      await Promise.resolve();
    });

    expect(key.value).toBe("new-user-key");
  });

  it("rolls Cloud model styling back when persistence rejects the optimistic selection", async () => {
    apiMocks.selectOpenaiModel.mockRejectedValueOnce("settings_save");
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
    fireEvent.click(screen.getByRole("button", { name: "OpenAI" }));
    const first = cloudModelOption("OpenAI Environment One");
    const second = cloudModelOption("OpenAI Environment Two");
    expectCloudModelSelected(first);

    fireEvent.click(second);

    await waitFor(() =>
      expect(screen.getByText(USER_ERROR_MESSAGES.settings_save)).toBeTruthy()
    );
    expectCloudModelSelected(first);
    expectCloudModelUnselected(second);
  });

  it("rolls Local model styling back when persistence rejects the optimistic selection", async () => {
    apiMocks.selectLocalModel.mockRejectedValueOnce("settings_save");
    render(
      <SettingsHarness
        initialSettings={localSettings(localInfos[1].path, "standard", "local")}
      />
    );
    await waitFor(() => expect(apiMocks.listLocalModels).toHaveBeenCalled());
    const first = localModelOption("Local Environment One");
    const second = localModelOption("Local Environment Two");
    await waitFor(() => expectLocalModelSelected(second));

    fireEvent.click(first);

    await waitFor(() =>
      expect(screen.getByText(USER_ERROR_MESSAGES.settings_save)).toBeTruthy()
    );
    expectLocalModelUnselected(first);
    expectLocalModelSelected(second);
  });

  it("keeps a newer provider event when an older model command returns a full stale Settings snapshot", async () => {
    let resolveModel!: (value: AppSettings) => void;
    apiMocks.selectOpenaiModel.mockImplementationOnce(
      () =>
        new Promise<AppSettings>((resolve) => {
          resolveModel = resolve;
        })
    );
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
    fireEvent.click(screen.getByRole("button", { name: "OpenAI" }));
    fireEvent.click(cloudModelOption("OpenAI Environment Two"));
    await waitFor(() => expect(apiMocks.selectOpenaiModel).toHaveBeenCalledTimes(1));

    act(() => tauriEventMocks.emit("settings:active-provider-changed", "groq"));
    await act(async () => {
      resolveModel({
        ...DEFAULT_SETTINGS,
        active_provider: "local",
        openai_model: "openai-environment-two",
        openai_model_kind: "standard",
      });
      await Promise.resolve();
    });

    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(
      screen.getByRole("radio", { name: "Groq" }).getAttribute("aria-checked")
    ).toBe("true");
  });

  it("does not let an older Settings provider response overwrite a newer committed provider event", async () => {
    let resolveProvider!: (value: AppSettings) => void;
    apiMocks.selectActiveProvider.mockImplementationOnce(
      () =>
        new Promise<AppSettings>((resolve) => {
          resolveProvider = resolve;
        })
    );
    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.click(screen.getByRole("radio", { name: "OpenAI" }));
    await waitFor(() => expect(apiMocks.selectActiveProvider).toHaveBeenCalledTimes(1));

    act(() => tauriEventMocks.emit("settings:active-provider-changed", "groq"));
    await act(async () => {
      resolveProvider({ ...DEFAULT_SETTINGS, active_provider: "openai" });
      await Promise.resolve();
    });

    expect(
      screen.getByRole("radio", { name: "Groq" }).getAttribute("aria-checked")
    ).toBe("true");
    expect(
      screen.getByRole("radio", { name: "OpenAI" }).getAttribute("aria-checked")
    ).toBe("false");
  });

  it("serializes rapid hotkey saves and restores the last successful shortcut when the newer one fails", async () => {
    let resolveFirst!: (value: AppSettings) => void;
    apiMocks.setHotkey
      .mockImplementationOnce(
        () =>
          new Promise<AppSettings>((resolve) => {
            resolveFirst = resolve;
          })
      )
      .mockRejectedValueOnce("invalid_shortcut");

    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    const input = screen.getByRole<HTMLInputElement>("textbox");

    fireEvent.focus(input);
    fireEvent.keyDown(input, { key: "k", ctrlKey: true, shiftKey: true });
    await waitFor(() => expect(apiMocks.setHotkey).toHaveBeenCalledTimes(1));

    fireEvent.focus(input);
    fireEvent.keyDown(input, { key: "l", ctrlKey: true, shiftKey: true });
    expect(apiMocks.setHotkey).toHaveBeenCalledTimes(1);

    await act(async () => {
      resolveFirst({ ...DEFAULT_SETTINGS, hotkey: "Ctrl+Shift+K" });
      await Promise.resolve();
    });

    await waitFor(() => expect(apiMocks.setHotkey).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(input.value).toBe("Ctrl+Shift+K"));
    expect(screen.getByText("⚠ Your shortcut could not be saved.")).toBeTruthy();
  });

  it("ignores an older Local model-list response after a newer selection refresh", async () => {
    let resolveInitialList!: (value: typeof localInfos) => void;
    apiMocks.listLocalModels
      .mockImplementationOnce(
        () =>
          new Promise<typeof localInfos>((resolve) => {
            resolveInitialList = resolve;
          })
      )
      .mockResolvedValueOnce(localInfos);

    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
    const second = localModelOption("Local Environment Two");
    fireEvent.click(second);

    await waitFor(() => expect(apiMocks.listLocalModels).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(apiMocks.selectLocalModel).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(within(second).getByText("Ready")).toBeTruthy());

    await act(async () => {
      resolveInitialList(
        localInfos.map((info) => ({ ...info, downloaded: false, size_bytes: null }))
      );
      await Promise.resolve();
    });

    expect(within(second).getByText("Ready")).toBeTruthy();
    expectLocalModelSelected(second);
  });

  it("does not let an older unresolved Local lookup issue a selection after a newer row click", async () => {
    let resolveFirstLookup!: (value: typeof localInfos) => void;
    let resolveSecondLookup!: (value: typeof localInfos) => void;
    apiMocks.listLocalModels
      .mockResolvedValueOnce([])
      .mockImplementationOnce(
        () =>
          new Promise<typeof localInfos>((resolve) => {
            resolveFirstLookup = resolve;
          })
      )
      .mockImplementationOnce(
        () =>
          new Promise<typeof localInfos>((resolve) => {
            resolveSecondLookup = resolve;
          })
      );

    render(<SettingsHarness initialSettings={DEFAULT_SETTINGS} />);
    await waitFor(() => expect(apiMocks.listLocalModels).toHaveBeenCalledTimes(1));

    const first = localModelOption("Local Environment One");
    const second = localModelOption("Local Environment Two");
    fireEvent.click(first);
    await waitFor(() => expect(apiMocks.listLocalModels).toHaveBeenCalledTimes(2));
    fireEvent.click(second);
    await waitFor(() => expect(apiMocks.listLocalModels).toHaveBeenCalledTimes(3));

    await act(async () => {
      resolveSecondLookup(localInfos);
      await Promise.resolve();
    });
    await waitFor(() =>
      expect(apiMocks.selectLocalModel).toHaveBeenCalledWith(localInfos[1].path)
    );

    await act(async () => {
      resolveFirstLookup(localInfos);
      await Promise.resolve();
    });

    expect(apiMocks.selectLocalModel).toHaveBeenCalledTimes(1);
    expectLocalModelUnselected(first);
    expectLocalModelSelected(second);
  });

  it("keeps Start disabled until an optimistic Cloud model selection is durably persisted", async () => {
    let resolveSelection!: (value: AppSettings) => void;
    apiMocks.getApiKey.mockImplementation((provider: string) =>
      Promise.resolve(provider === "openai" ? "stored-openai-key" : "")
    );
    apiMocks.selectOpenaiModel.mockImplementationOnce(
      () =>
        new Promise<AppSettings>((resolve) => {
          resolveSelection = resolve;
        })
    );
    const initialSettings: AppSettings = {
      ...DEFAULT_SETTINGS,
      active_provider: "openai",
      openai_model: "",
      openai_model_kind: "",
    };

    render(<SettingsHarness initialSettings={initialSettings} />);
    fireEvent.click(screen.getByRole("button", { name: "OpenAI" }));
    const key = await screen.findByPlaceholderText<HTMLInputElement>("Your API Key");
    await waitFor(() => expect(key.value).toBe("stored-openai-key"));
    const start = screen.getByRole<HTMLButtonElement>("button", { name: "Start" });
    expect(start.disabled).toBe(true);

    fireEvent.click(cloudModelOption("OpenAI Environment One"));
    await waitFor(() => expect(apiMocks.selectOpenaiModel).toHaveBeenCalledTimes(1));
    expect(start.disabled).toBe(true);

    await act(async () => {
      resolveSelection({
        ...initialSettings,
        openai_model: "openai-environment-one",
        openai_model_kind: "standard",
      });
      await Promise.resolve();
    });

    await waitFor(() => expect(start.disabled).toBe(false));
  });

  it("keeps Start disabled until an optimistic Local model selection is durably persisted", async () => {
    let resolveSelection!: (value: AppSettings) => void;
    apiMocks.selectLocalModel.mockImplementationOnce(
      () =>
        new Promise<AppSettings>((resolve) => {
          resolveSelection = resolve;
        })
    );
    const initialSettings = localSettings("", "", "local");

    render(<SettingsHarness initialSettings={initialSettings} />);
    await waitFor(() => expect(apiMocks.listLocalModels).toHaveBeenCalled());
    const start = screen.getByRole<HTMLButtonElement>("button", { name: "Start" });
    expect(start.disabled).toBe(true);

    fireEvent.click(localModelOption("Local Environment Two"));
    await waitFor(() => expect(apiMocks.selectLocalModel).toHaveBeenCalledTimes(1));
    expect(start.disabled).toBe(true);

    await act(async () => {
      resolveSelection({
        ...initialSettings,
        local_model_path: localInfos[1].path,
        local_model_kind: "standard",
      });
      await Promise.resolve();
    });

    await waitFor(() => expect(start.disabled).toBe(false));
  });

  it("routes a native close request through the same flush-and-hide path", async () => {
    render(
      <SettingsHarness
        initialSettings={{
          ...DEFAULT_SETTINGS,
          onboarding_complete: true,
        }}
      />
    );

    tauriEventMocks.emit("settings:close-requested", undefined);

    await waitFor(() => expect(windowMocks.hide).toHaveBeenCalledTimes(1));
  });
});
