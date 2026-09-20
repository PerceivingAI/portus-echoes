import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Cloud,
  Cpu,
  Settings as SettingsIcon,
  ShieldCheck,
  Zap,
} from "lucide-react";
import { AlertBanner } from "../components/AlertBanner";
import { ConfirmModal } from "../components/ConfirmModal";
import type {
  AppSettings,
  ProviderId,
  SettingsNavigationTab,
  UserErrorCode,
} from "../lib/types";
import { SettingsHeader } from "./components/SettingsHeader";
import { SettingsNav } from "./components/SettingsNav";
import { useApplicationErrors } from "./hooks/useApplicationErrors";
import { useCloudProvider } from "./hooks/useCloudProvider";
import { useDiagnostics } from "./hooks/useDiagnostics";
import { useLocalModels } from "./hooks/useLocalModels";
import { useSettingsLifecycle } from "./hooks/useSettingsLifecycle";
import { useFocusedHotkeyInput } from "./hooks/useFocusedHotkeyInput";
import { DiagnosticsPane } from "./panes/DiagnosticsPane";
import { GroqPane } from "./panes/GroqPane";
import { LocalPane } from "./panes/LocalPane";
import { OpenaiPane } from "./panes/OpenaiPane";
import { SettingsPane } from "./panes/SettingsPane";

interface StepSetupTabsProps {
  settings: AppSettings;
  onSaved: (settings: AppSettings) => void;
  onFinish: () => Promise<void>;
  initialTab?: TabId;
  navigationTab?: SettingsNavigationTab;
  initialErrorCode?: UserErrorCode;
}

type TabId = "settings" | "local" | "openai" | "groq" | "diagnostics";

const NAVIGATION_ITEMS = [
  { id: "settings", label: "Settings", Icon: SettingsIcon },
  { id: "local", label: "Local", Icon: Cpu },
  { id: "openai", label: "OpenAI", Icon: Cloud },
  { id: "groq", label: "Groq", Icon: Zap },
  { id: "diagnostics", label: "Diagnostics", Icon: ShieldCheck },
] as const;

const ACTIVE_PROVIDER_OPTIONS: ReadonlyArray<{
  id: ProviderId;
  label: string;
}> = [
  { id: "local", label: "Local" },
  { id: "openai", label: "OpenAI" },
  { id: "groq", label: "Groq" },
];

export function StepSetupTabs({
  settings,
  onSaved,
  onFinish,
  initialTab,
  navigationTab,
  initialErrorCode,
}: StepSetupTabsProps) {
  const [activeTab, setActiveTab] = useState<TabId>(
    initialTab ?? settings.active_provider
  );
  const { error, setError, statusMessage } = useApplicationErrors(initialErrorCode);
  const settingsRef = useRef(settings);
  useFocusedHotkeyInput();

  const applySettings = useCallback(
    (patch: Partial<AppSettings>) => {
      const next = { ...settingsRef.current, ...patch };
      settingsRef.current = next;
      onSaved(next);
    },
    [onSaved]
  );

  useEffect(() => {
    settingsRef.current = settings;
  }, [settings]);

  const openai = useCloudProvider({
    provider: "openai",
    settings,
    applySettings,
    setError,
  });
  const groq = useCloudProvider({
    provider: "groq",
    settings,
    applySettings,
    setError,
  });
  const local = useLocalModels({ settings, applySettings, setError });
  const diagnostics = useDiagnostics();

  const flushDirtyFields = useCallback(async () => {
    await Promise.all([
      openai.flushDirtyFields(),
      groq.flushDirtyFields(),
      local.flushDirtyFields(),
    ]);
  }, [
    groq.flushDirtyFields,
    local.flushDirtyFields,
    openai.flushDirtyFields,
  ]);

  const lifecycle = useSettingsLifecycle({
    settingsRef,
    applySettings,
    setError,
    flushDirtyFields,
    onFinish,
  });

  useEffect(() => {
    if (navigationTab) setActiveTab(navigationTab);
  }, [navigationTab]);

  const isAnyConfigured =
    openai.isConfigured || groq.isConfigured || local.isConfigured;

  const handleDrag = (event: React.MouseEvent) => {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement;
    if (
      target.closest("input") ||
      target.closest("button") ||
      target.closest(".model-card") ||
      target.closest("#hotkey-btn")
    ) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    getCurrentWindow().startDragging().catch(() => {});
  };

  return (
    <div className="relative flex h-full w-full flex-col overflow-hidden select-none bg-hud-panel text-slate-100">
      <SettingsHeader
        onClose={() => void lifecycle.handleFinish()}
        onDrag={handleDrag}
      />

      <div className="flex min-h-0 flex-1">
        <SettingsNav
          activeTab={activeTab}
          navigationItems={NAVIGATION_ITEMS}
          onSelectTab={setActiveTab}
          onboardingComplete={settings.onboarding_complete}
          isAnyConfigured={isAnyConfigured}
          busy={lifecycle.busy}
          onFinish={lifecycle.handleFinish}
        />

        <div className="relative min-w-0 min-h-0 flex-1 flex flex-col bg-hud-content">
          <main
            id="pane-container"
            className="min-w-0 min-h-0 flex-1 overflow-y-auto px-6 pt-[23px] pb-10 flex flex-col justify-start space-y-6"
          >
            {activeTab === "local" && (
              <LocalPane
                localInfos={local.localInfos}
                isLocalModelSelected={local.isLocalModelSelected}
                isCustomLocal={local.isCustomLocal}
                downloadState={local.downloadState}
                customPathInput={local.customPathInput}
                onSelectStandardModel={local.selectStandardModel}
                onSelectCustomModel={() => void local.selectCustomModel()}
                onChangeCustomPath={local.changeCustomPath}
                onSaveCustomPath={local.saveCustomPath}
                onBrowseCustomPath={local.pickCustomBin}
                onStartDownload={local.startDownload}
                onCancelDownload={local.cancelActiveDownload}
                onOpenDeleteModal={local.openDeleteModal}
              />
            )}

            {activeTab === "openai" && (
              <OpenaiPane
                selection={openai.selection}
                customModelInput={openai.customModelInput}
                apiKey={openai.apiKey}
                showKey={openai.showKey}
                onSelectStandardModel={openai.selectStandardModel}
                onSelectCustomModel={openai.selectCustomModel}
                onChangeCustomModel={openai.changeCustomModel}
                onSaveCustomModel={openai.saveCustomModel}
                onChangeKey={openai.changeKey}
                onSaveKey={openai.saveKey}
                onToggleShowKey={openai.toggleShowKey}
              />
            )}

            {activeTab === "groq" && (
              <GroqPane
                selection={groq.selection}
                customModelInput={groq.customModelInput}
                apiKey={groq.apiKey}
                showKey={groq.showKey}
                onSelectStandardModel={groq.selectStandardModel}
                onSelectCustomModel={groq.selectCustomModel}
                onChangeCustomModel={groq.changeCustomModel}
                onSaveCustomModel={groq.saveCustomModel}
                onChangeKey={groq.changeKey}
                onSaveKey={groq.saveKey}
                onToggleShowKey={groq.toggleShowKey}
              />
            )}

            {activeTab === "settings" && (
              <SettingsPane
                settings={settings}
                activeProviderOptions={ACTIVE_PROVIDER_OPTIONS}
                onSelectProvider={lifecycle.selectProvider}
                hasError={Boolean(error)}
                onChangeHotkey={lifecycle.changeHotkey}
                hotkeyWarning={lifecycle.hotkeyWarning}
              />
            )}

            {activeTab === "diagnostics" && (
              <DiagnosticsPane
                diagnostics={diagnostics.diagnostics}
                runningDiagnostics={diagnostics.runningDiagnostics}
                completedRunSerial={diagnostics.completedDiagnosticsRuns}
                failedRunSerial={diagnostics.failedDiagnosticsRuns}
                onRefresh={diagnostics.refreshDiagnostics}
              />
            )}
          </main>

          {error ? (
            <div className="absolute bottom-4 left-6 right-6 pointer-events-none z-10">
              <AlertBanner message={error} />
            </div>
          ) : statusMessage ? (
            <div className="absolute bottom-4 left-6 right-6 pointer-events-none z-10">
              <p
                data-testid="status-banner"
                className={`text-xs ${
                  statusMessage.variant === "copied"
                    ? "text-accent font-medium"
                    : "text-slate-400"
                }`}
              >
                {statusMessage.text}
              </p>
            </div>
          ) : null}
        </div>
      </div>

      <ConfirmModal
        isOpen={Boolean(local.deleteCandidate)}
        message="Are you sure you want to delete the model?"
        isPending={local.deletePending}
        cancelRef={local.cancelDeleteRef}
        confirmRef={local.confirmDeleteRef}
        onConfirm={local.confirmDelete}
        onCancel={local.closeDeleteModal}
        onKeyDown={local.handleDeleteModalKeyDown}
      />
    </div>
  );
}
