import type { AppSettings } from "./types";

/** Build-time curated model slots from .env (docs/DEFAULTS.md). */

export type CloudRoute = "completed" | "live";

export interface CloudModel {
  id: string;
  label: string;
  route: CloudRoute;
}

export interface LocalModel {
  downloadPath: string;
  label: string;
}

export type StandardModelSlots<T> = readonly [T | null, T | null];

type ModelGuard<T> = (value: unknown) => value is T;


const isCloudModel: ModelGuard<CloudModel> = (value): value is CloudModel =>
  typeof value === "object" &&
  value !== null &&
  "id" in value &&
  typeof value.id === "string" &&
  "label" in value &&
  typeof value.label === "string" &&
  "route" in value &&
  (value.route === "completed" || value.route === "live");

const isLocalModel: ModelGuard<LocalModel> = (value): value is LocalModel =>
  typeof value === "object" &&
  value !== null &&
  "downloadPath" in value &&
  typeof value.downloadPath === "string" &&
  "label" in value &&
  typeof value.label === "string";

function parseModelSlots<T>(
  raw: string | undefined,
  variableName: string,
  isModel: ModelGuard<T>
): StandardModelSlots<T> {
  if (!raw?.trim()) return [null, null];

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error(`${variableName} must be valid JSON`);
  }

  if (!Array.isArray(parsed)) {
    throw new Error(`${variableName} must be a JSON array`);
  }
  if (parsed.length > 2) {
    throw new Error(`${variableName} supports at most two standard models`);
  }
  if (!parsed.every(isModel)) {
    throw new Error(`${variableName} contains an invalid model entry`);
  }

  return [parsed[0] ?? null, parsed[1] ?? null];
}

export const parseCloudModelSlots = (
  raw: string | undefined,
  variableName: string
): StandardModelSlots<CloudModel> => {
  const slots = parseModelSlots(raw, variableName, isCloudModel);
  const ids = new Set<string>();
  for (const model of slots) {
    if (model && ids.has(model.id)) {
      throw new Error(
        `${variableName} contains duplicate Standard model id '${model.id}'`
      );
    }
    if (model) ids.add(model.id);
  }
  return slots;
};

export const parseLocalModelSlots = (
  raw: string | undefined,
  variableName: string
) => parseModelSlots(raw, variableName, isLocalModel);

export const LOCAL_MODEL_SLOTS = parseLocalModelSlots(
  import.meta.env.LOCAL_MODELS,
  "LOCAL_MODELS"
);

export const OPENAI_MODEL_SLOTS = parseCloudModelSlots(
  import.meta.env.OPENAI_MODELS,
  "OPENAI_MODELS"
);

export const GROQ_MODEL_SLOTS = parseCloudModelSlots(
  import.meta.env.GROQ_MODELS,
  "GROQ_MODELS"
);

export const DEFAULT_HOTKEY = "Ctrl+Alt+Space";

export const DEFAULT_SETTINGS: AppSettings = {
  active_provider: "local",
  openai_model: OPENAI_MODEL_SLOTS[0]?.id ?? "",
  openai_model_kind: OPENAI_MODEL_SLOTS[0] ? "standard" : "",
  openai_custom_model: "",
  groq_model: GROQ_MODEL_SLOTS[0]?.id ?? "",
  groq_model_kind: GROQ_MODEL_SLOTS[0] ? "standard" : "",
  groq_custom_model: "",
  local_model: LOCAL_MODEL_SLOTS[0]?.label ?? "",
  local_model_path: "",
  local_model_kind: LOCAL_MODEL_SLOTS[0] ? "standard" : "",
  local_custom_model_path: "",
  language: "auto",
  hotkey: "Ctrl+Alt+Space",
  onboarding_complete: false,
};
