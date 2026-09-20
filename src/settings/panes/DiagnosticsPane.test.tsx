// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DiagnosticsStorageData } from "../../lib/types";
import { DiagnosticsPane } from "./DiagnosticsPane";

const mockDiagnostics: DiagnosticsStorageData = {
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
      passed: "fail",
      name: "A very long operating-system microphone name that should stay complete in the title",
      specs: "None/Not Valid",
    },
    transcription_access: {
      passed: "pass",
      function: "send_input",
      access_type: "cursor",
    },
  },
};

const neverRun: DiagnosticsStorageData = {
  last_scan_display: "",
  snapshot: null,
};

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("DiagnosticsPane component", () => {
  it("renders exact values with status icons only at the four group headings", () => {
    const { container } = render(
      <DiagnosticsPane
        diagnostics={mockDiagnostics}
        runningDiagnostics={false}
        completedRunSerial={0}
        failedRunSerial={0}
        onRefresh={() => {}}
      />
    );

    expect(screen.getByText("Model Path: Validated")).toBeDefined();
    expect(screen.getByText("Model File: Validated")).toBeDefined();
    expect(screen.getByText("OpenAI API Key: Validated")).toBeDefined();
    expect(screen.getByText("OpenAI Model: Validated")).toBeDefined();
    expect(screen.getByText("Groq API Key: None/Not Valid")).toBeDefined();
    expect(screen.getByText("Groq Model: None/Not Valid")).toBeDefined();
    expect(
      screen.getByText(
        "Name: A very long operating-system microphone name that should stay complete in the title"
      )
    ).toBeDefined();
    expect(screen.getByText("Specs: None/Not Valid")).toBeDefined();
    expect(screen.getByText("Function: Native")).toBeDefined();
    expect(screen.getByText("Type: Cursor")).toBeDefined();

    expect(container.querySelectorAll("[data-diagnostic-group-heading]")).toHaveLength(4);
    expect(container.querySelectorAll("[data-diagnostic-group-icon]")).toHaveLength(4);
    expect(container.querySelectorAll("[data-diagnostic-group-icon='pass']")).toHaveLength(3);
    expect(container.querySelectorAll("[data-diagnostic-group-icon='fail']")).toHaveLength(1);
    expect(
      container.querySelector(
        "[data-diagnostic-group-heading='Cloud Provider'] [data-diagnostic-group-icon='pass']"
      )
    ).not.toBeNull();
    const cloudHeading = container.querySelector(
      "[data-diagnostic-group-heading='Cloud Provider']"
    ) as HTMLElement;
    expect(cloudHeading.firstElementChild?.tagName).toBe("H3");
    expect(
      cloudHeading.lastElementChild?.querySelector("[data-diagnostic-group-icon='pass']")
    ).not.toBeNull();

    const rows = container.querySelectorAll("[data-diagnostic-item-row]");
    expect(rows).toHaveLength(10);
    rows.forEach((row) => expect(row.querySelector("svg")).toBeNull());
  });

  it("uses a real spacer before Groq and places desktop status in the Groq Model row", () => {
    const { container } = render(
      <DiagnosticsPane
        diagnostics={mockDiagnostics}
        runningDiagnostics={false}
        completedRunSerial={0}
        failedRunSerial={0}
        onRefresh={() => {}}
      />
    );

    const gap = container.querySelector("[data-diagnostic-provider-gap]") as HTMLElement;
    expect(gap).not.toBeNull();
    expect(gap.className).toContain("h-2");

    const cloud = container.querySelector("[data-diagnostic-cloud-group]") as HTMLElement;
    const access = container.querySelector("[data-diagnostic-access-group]") as HTMLElement;
    const status = container.querySelector("[data-diagnostic-status]") as HTMLElement;

    expect(cloud.className).toContain("sm:row-start-2");
    expect(access.className).toContain("sm:row-start-2");
    expect(status.className).not.toContain("absolute");
    expect(status.textContent).toBe("Last scan: 21 Mar 2026");

    const cloudChildren = Array.from(cloud.children);
    const accessChildren = Array.from(access.children);
    expect(cloudChildren[5]?.textContent).toContain("Groq Model: None/Not Valid");
    expect(accessChildren[5]).toBe(status);
  });

  it("renders all ten neutral values and no status icons before the first run", () => {
    const { container } = render(
      <DiagnosticsPane
        diagnostics={neverRun}
        runningDiagnostics={false}
        completedRunSerial={0}
        failedRunSerial={0}
        onRefresh={() => {}}
      />
    );

    for (const text of [
      "Model Path: -",
      "Model File: -",
      "OpenAI API Key: -",
      "OpenAI Model: -",
      "Groq API Key: -",
      "Groq Model: -",
      "Name: -",
      "Specs: -",
      "Function: -",
      "Type: -",
    ]) {
      expect(screen.getByText(text)).toBeDefined();
    }
    expect(screen.getByText("Last scan: -")).toBeDefined();
    expect(container.querySelectorAll("[data-diagnostic-group-heading]")).toHaveLength(4);
    expect(container.querySelectorAll("[data-diagnostic-group-icon]")).toHaveLength(0);
  });

  it("keeps the Run Diagnostics button geometry, styling, label, and disabled protection", () => {
    const handleRefresh = vi.fn();
    const { container, rerender } = render(
      <DiagnosticsPane
        diagnostics={mockDiagnostics}
        runningDiagnostics={false}
        completedRunSerial={0}
        failedRunSerial={0}
        onRefresh={handleRefresh}
      />
    );

    const button = container.querySelector(
      "button[title='Run Diagnostics']"
    ) as HTMLButtonElement;
    expect(button).not.toBeNull();
    expect(button.getAttribute("aria-label")).toBe("Run Diagnostics");
    expect(button.className).toBe(
      "flex h-7 w-7 items-center justify-center rounded-md text-slate-400 hover:text-slate-200 transition-colors focus:outline-none cursor-pointer shrink-0 disabled:opacity-40 -translate-x-[5px]"
    );

    fireEvent.click(button);
    expect(handleRefresh).toHaveBeenCalledTimes(1);

    rerender(
      <DiagnosticsPane
        diagnostics={mockDiagnostics}
        runningDiagnostics={true}
        completedRunSerial={0}
        failedRunSerial={0}
        onRefresh={handleRefresh}
      />
    );
    expect(button.disabled).toBe(true);
    fireEvent.click(button);
    expect(handleRefresh).toHaveBeenCalledTimes(1);
  });

  it("keeps every result on one ellipsized line and preserves the full text in title", () => {
    render(
      <DiagnosticsPane
        diagnostics={mockDiagnostics}
        runningDiagnostics={false}
        completedRunSerial={0}
        failedRunSerial={0}
        onRefresh={() => {}}
      />
    );

    const line = screen.getByText(
      "Name: A very long operating-system microphone name that should stay complete in the title"
    );
    expect(line.className).toContain("truncate");
    expect(line.className).toContain("whitespace-nowrap");
    expect(line.className).toContain("text-xs");
    expect(line.className).toContain("text-slate-400");
    expect(line.getAttribute("title")).toBe(line.textContent);
  });

  it("hides prior group icons and renders all ten values as neutral while a new run is active", () => {
    const { container } = render(
      <DiagnosticsPane
        diagnostics={mockDiagnostics}
        runningDiagnostics={true}
        completedRunSerial={0}
        failedRunSerial={0}
        onRefresh={() => {}}
      />
    );

    expect(screen.getByText("Running...")).toBeDefined();
    for (const text of [
      "Model Path: -",
      "Model File: -",
      "OpenAI API Key: -",
      "OpenAI Model: -",
      "Groq API Key: -",
      "Groq Model: -",
      "Name: -",
      "Specs: -",
      "Function: -",
      "Type: -",
    ]) {
      expect(screen.getByText(text)).toBeDefined();
    }
    expect(container.querySelectorAll("[data-diagnostic-group-icon]")).toHaveLength(0);
    expect(screen.queryByText("Model Path: Validated")).toBeNull();
    expect(screen.queryByText("OpenAI API Key: Validated")).toBeNull();
    expect(screen.queryByText("Function: Native")).toBeNull();
  });

  it("keeps all ten neutral values visible during a first-ever run", () => {
    const { container } = render(
      <DiagnosticsPane
        diagnostics={neverRun}
        runningDiagnostics={true}
        completedRunSerial={0}
        failedRunSerial={0}
        onRefresh={() => {}}
      />
    );

    expect(screen.getByText("Running...")).toBeDefined();
    expect(container.querySelectorAll("[data-diagnostic-item-row]")).toHaveLength(10);
    for (const text of [
      "Model Path: -",
      "Model File: -",
      "OpenAI API Key: -",
      "OpenAI Model: -",
      "Groq API Key: -",
      "Groq Model: -",
      "Name: -",
      "Specs: -",
      "Function: -",
      "Type: -",
    ]) {
      expect(screen.getByText(text)).toBeDefined();
    }
    expect(container.querySelectorAll("[data-diagnostic-group-icon]")).toHaveLength(0);
  });

  it("shows Running, then Ready only after a successful completed snapshot, then settles", () => {
    vi.useFakeTimers();
    const { rerender } = render(
      <DiagnosticsPane
        diagnostics={mockDiagnostics}
        runningDiagnostics={true}
        completedRunSerial={0}
        failedRunSerial={0}
        onRefresh={() => {}}
      />
    );

    expect(screen.getByText("Running...")).toBeDefined();

    rerender(
      <DiagnosticsPane
        diagnostics={mockDiagnostics}
        runningDiagnostics={false}
        completedRunSerial={1}
        failedRunSerial={0}
        onRefresh={() => {}}
      />
    );
    expect(screen.getByText("Ready")).toBeDefined();

    act(() => {
      vi.advanceTimersByTime(1500);
    });
    expect(screen.getByText("Last scan: 21 Mar 2026")).toBeDefined();
  });

  it("shows Unable to proceed after failure, then restores the previous scan", () => {
    vi.useFakeTimers();
    const { container, rerender } = render(
      <DiagnosticsPane
        diagnostics={mockDiagnostics}
        runningDiagnostics={true}
        completedRunSerial={0}
        failedRunSerial={0}
        onRefresh={() => {}}
      />
    );
    expect(screen.getByText("Running...")).toBeDefined();

    rerender(
      <DiagnosticsPane
        diagnostics={mockDiagnostics}
        runningDiagnostics={false}
        completedRunSerial={0}
        failedRunSerial={1}
        onRefresh={() => {}}
      />
    );

    expect(screen.queryByText("Ready")).toBeNull();
    expect(screen.getByText("Unable to proceed")).toBeDefined();
    expect(screen.getByText("OpenAI Model: -")).toBeDefined();
    expect(screen.getByText("Name: -")).toBeDefined();
    expect(container.querySelectorAll("[data-diagnostic-group-icon]")).toHaveLength(0);
    expect(screen.queryByText("Last scan: 21 Mar 2026")).toBeNull();

    act(() => {
      vi.advanceTimersByTime(1500);
    });
    expect(screen.getByText("Last scan: 21 Mar 2026")).toBeDefined();
    expect(screen.getByText("OpenAI Model: Validated")).toBeDefined();
    expect(screen.getByText("Name: A very long operating-system microphone name that should stay complete in the title")).toBeDefined();
    expect(container.querySelectorAll("[data-diagnostic-group-icon]")).toHaveLength(4);
  });
});
