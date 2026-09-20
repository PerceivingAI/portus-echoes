import { invoke } from "@tauri-apps/api/core";
import type {
  AppSettings,
  DiagnosticsStorageData,
  LocalModelInfo,
  ProviderId,
  ProviderStatus,
  RecordingState,
  SettingsNavigationState,
} from "./types";

export interface FocusedHotkeyKeyEvent {
  key: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
  pressed: boolean;
  repeat: boolean;
}

export const getSettings = () => invoke<AppSettings>("get_settings");
export const getRecordingState = () =>
  invoke<RecordingState>("get_recording_state");
export const getSettingsNavigation = () =>
  invoke<SettingsNavigationState | null>("get_settings_navigation");
export const selectActiveProvider = (provider: ProviderId) =>
  invoke<AppSettings>("select_active_provider", { provider });
export const selectOpenaiModel = (model: string) =>
  invoke<AppSettings>("select_openai_model", { model });
export const selectOpenaiCustomModel = () =>
  invoke<AppSettings>("select_openai_custom_model");
export const setOpenaiCustomModel = (model: string) =>
  invoke<AppSettings>("set_openai_custom_model", { model });
export const selectGroqModel = (model: string) =>
  invoke<AppSettings>("select_groq_model", { model });
export const selectGroqCustomModel = () =>
  invoke<AppSettings>("select_groq_custom_model");
export const setGroqCustomModel = (model: string) =>
  invoke<AppSettings>("set_groq_custom_model", { model });
export const selectLocalModel = (path: string) =>
  invoke<AppSettings>("select_local_model", { path });
export const selectLocalCustomModel = () =>
  invoke<AppSettings>("select_local_custom_model");
export const setLocalCustomModelPath = (path: string) =>
  invoke<AppSettings>("set_local_custom_model_path", { path });
export const setHotkey = (hotkey: string) =>
  invoke<AppSettings>("set_hotkey", { hotkey });
export const sendFocusedHotkeyKeyEvent = (event: FocusedHotkeyKeyEvent) =>
  invoke<boolean>("focused_hotkey_key_event", { event });
export const setHotkeyCaptureActive = (active: boolean) =>
  invoke<void>("set_hotkey_capture_active", { active });
export const resetFocusedHotkeyInput = () =>
  invoke<void>("reset_focused_hotkey_input");
export const completeOnboarding = () =>
  invoke<AppSettings>("complete_onboarding");
export const setApiKey = (provider: ProviderId, key: string) =>
  invoke<void>("set_api_key", { provider, key });

export const getApiKey = (provider: ProviderId) =>
  invoke<string>("get_api_key", { provider });

export const deleteApiKey = (provider: ProviderId) =>
  invoke<void>("delete_api_key", { provider });

export const getProviderStatus = () => invoke<ProviderStatus>("get_provider_status");

export const isHotkeyRegistered = (combo: string) =>
  invoke<boolean>("is_hotkey_registered", { combo });

export const isHotkeyActive = () => invoke<boolean>("is_hotkey_active");

export const getDiagnosticsState = () =>
  invoke<DiagnosticsStorageData>("get_diagnostics_state");

export const runDiagnostics = () =>
  invoke<DiagnosticsStorageData>("run_diagnostics");
export const listLocalModels = (downloadPaths: string[]) =>
  invoke<LocalModelInfo[]>("list_local_models", { downloadPaths });

export const downloadModel = (downloadPath: string) =>
  invoke<void>("download_model", { downloadPath });

export const cancelDownload = (downloadPath: string) =>
  invoke<void>("cancel_download", { downloadPath });

export const deleteModel = (downloadPath: string) =>
  invoke<AppSettings>("delete_model", { downloadPath });
