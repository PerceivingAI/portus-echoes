import { describe, expect, it } from "vitest";
import {
  DEFAULT_SETTINGS,
  GROQ_MODEL_SLOTS,
  LOCAL_MODEL_SLOTS,
  OPENAI_MODEL_SLOTS,
  parseCloudModelSlots,
  parseLocalModelSlots,
} from "./constants";

describe("build-time model map", () => {
  it("returns two empty slots for a missing or empty variable", () => {
    expect(parseCloudModelSlots(undefined, "VITE_TEST_MODELS")).toEqual([
      null,
      null,
    ]);
    expect(parseCloudModelSlots("[]", "VITE_TEST_MODELS")).toEqual([
      null,
      null,
    ]);
  });

  it("keeps one cloud entry and leaves slot two empty", () => {
    expect(
      parseCloudModelSlots(
        '[{"id":"environment-model","label":"Environment Label","route":"live"}]',
        "VITE_TEST_MODELS"
      )
    ).toEqual([
      { id: "environment-model", label: "Environment Label", route: "live" },
      null,
    ]);
  });

  it("defaults the active provider to Local", () => {
    expect(DEFAULT_SETTINGS.active_provider).toBe("local");
  });

  it("derives slot-one Standard defaults for Local and both Cloud providers", () => {
    expect(DEFAULT_SETTINGS.openai_model).toBe(
      OPENAI_MODEL_SLOTS[0]?.id ?? ""
    );
    expect(DEFAULT_SETTINGS.openai_model_kind).toBe(
      OPENAI_MODEL_SLOTS[0] ? "standard" : ""
    );
    expect(DEFAULT_SETTINGS.groq_model).toBe(GROQ_MODEL_SLOTS[0]?.id ?? "");
    expect(DEFAULT_SETTINGS.groq_model_kind).toBe(
      GROQ_MODEL_SLOTS[0] ? "standard" : ""
    );
    expect(DEFAULT_SETTINGS.local_model_path).toBe("");
    expect(DEFAULT_SETTINGS.local_model_kind).toBe(
      LOCAL_MODEL_SLOTS[0] ? "standard" : ""
    );
  });

  it("keeps two Local paths and labels exactly", () => {
    const raw = JSON.stringify([
      {
        downloadPath: "custom-scheme://host/path/model-one.bin?raw=1",
        label: "First Environment Model",
      },
      {
        downloadPath: "not-a-valid-url",
        label: "Second Environment Model",
      },
    ]);

    expect(parseLocalModelSlots(raw, "VITE_TEST_MODELS")).toEqual([
      {
        downloadPath: "custom-scheme://host/path/model-one.bin?raw=1",
        label: "First Environment Model",
      },
      {
        downloadPath: "not-a-valid-url",
        label: "Second Environment Model",
      },
    ]);
  });

  it("rejects more than two standard entries", () => {
    const raw = JSON.stringify([
      { id: "one", label: "One", route: "completed" },
      { id: "two", label: "Two", route: "live" },
      { id: "three", label: "Three", route: "completed" },
    ]);

    expect(() => parseCloudModelSlots(raw, "OPENAI_MODELS")).toThrow(
      "OPENAI_MODELS supports at most two standard models"
    );
  });

  it("rejects duplicate Standard Cloud model IDs even when route metadata differs", () => {
    expect(() =>
      parseCloudModelSlots(
        '[{"id":"same","label":"Completed","route":"completed"},{"id":"same","label":"Live","route":"live"}]',
        "OPENAI_MODELS"
      )
    ).toThrow("OPENAI_MODELS contains duplicate Standard model id 'same'");
  });

  it("rejects malformed JSON and invalid entry shapes", () => {
    expect(() =>
      parseCloudModelSlots("not-json", "GROQ_MODELS")
    ).toThrow("GROQ_MODELS must be valid JSON");
    expect(() => parseCloudModelSlots("{}", "GROQ_MODELS")).toThrow(
      "GROQ_MODELS must be a JSON array"
    );
    expect(() =>
      parseCloudModelSlots(
        '[{"id":"missing-label"}]',
        "GROQ_MODELS"
      )
    ).toThrow("GROQ_MODELS contains an invalid model entry");
    expect(() =>
      parseCloudModelSlots(
        '[{"id":"missing-route","label":"Missing Route"}]',
        "GROQ_MODELS"
      )
    ).toThrow("GROQ_MODELS contains an invalid model entry");
    expect(() =>
      parseCloudModelSlots(
        '[{"id":"bad-route","label":"Bad Route","route":"streaming"}]',
        "GROQ_MODELS"
      )
    ).toThrow("GROQ_MODELS contains an invalid model entry");
  });
});
