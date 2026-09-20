import { isUserErrorCode, type UserErrorCode } from "./types";

/**
 * Approved GUI error catalog and exact stable-code lookup.
 * Raw backend/OS/IPC exception strings must never reach the UI.
 * Message text is never used as routing data.
 */

export const APPROVED_ERROR_MESSAGES = {
  CREDENTIAL_STORAGE: "Unable to access the API key storage",
  INVALID_SHORTCUT: "Invalid shortcut combination",
  MODEL_DOWNLOAD: "Unable to download the model",
  NETWORK_CONNECTION: "Unable to connect to the cloud transcription service",
  CLOUD_SERVICE: "Cloud transcription service unavailable",
  LOCAL_MODEL_NOT_FOUND: "Model file not found",
  SETTINGS_SAVE: "Unable to save settings",
  UNSUPPORTED_WHISPER_MODEL: "The selected file is not a supported Whisper.cpp model",
  INSUFFICIENT_RAM: "Not enough RAM to load the model",
  API_KEY_REJECTED: "API key rejected",
  RATE_LIMIT: "Rate limit reached",
  RECORDING_TOO_LARGE: "Recording is too large",
  DELIVERY: "Delivery error.",
  UNEXPECTED: "An unexpected error occurred",
} as const;

export const USER_ERROR_MESSAGES = {
  credential_storage: APPROVED_ERROR_MESSAGES.CREDENTIAL_STORAGE,
  invalid_shortcut: APPROVED_ERROR_MESSAGES.INVALID_SHORTCUT,
  model_download: APPROVED_ERROR_MESSAGES.MODEL_DOWNLOAD,
  network_connection: APPROVED_ERROR_MESSAGES.NETWORK_CONNECTION,
  cloud_service: APPROVED_ERROR_MESSAGES.CLOUD_SERVICE,
  local_model_not_found: APPROVED_ERROR_MESSAGES.LOCAL_MODEL_NOT_FOUND,
  settings_save: APPROVED_ERROR_MESSAGES.SETTINGS_SAVE,
  unsupported_whisper_model: APPROVED_ERROR_MESSAGES.UNSUPPORTED_WHISPER_MODEL,
  insufficient_ram: APPROVED_ERROR_MESSAGES.INSUFFICIENT_RAM,
  api_key_rejected: APPROVED_ERROR_MESSAGES.API_KEY_REJECTED,
  rate_limit: APPROVED_ERROR_MESSAGES.RATE_LIMIT,
  recording_too_large: APPROVED_ERROR_MESSAGES.RECORDING_TOO_LARGE,
  delivery: APPROVED_ERROR_MESSAGES.DELIVERY,
  unexpected: APPROVED_ERROR_MESSAGES.UNEXPECTED,
} satisfies Record<UserErrorCode, string>;

export function errorMessageForCode(code: unknown): string {
  return USER_ERROR_MESSAGES[isUserErrorCode(code) ? code : "unexpected"];
}
