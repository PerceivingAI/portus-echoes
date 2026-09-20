import { HotkeyRecorder } from "../../components/HotkeyRecorder";
import { PaneHeader } from "../../components/PaneHeader";
import { RadioOptionRow } from "../../components/RadioOptionRow";
import { SectionLabel } from "../../components/SectionLabel";
import type { AppSettings, ProviderId } from "../../lib/types";

export interface SettingsPaneProps {
  settings: AppSettings;
  activeProviderOptions: ReadonlyArray<{ id: ProviderId; label: string }>;
  onSelectProvider: (provider: ProviderId) => void;
  onChangeHotkey: (combo: string) => void;
  hotkeyWarning?: string | null;
  hasError?: boolean;
}

export function SettingsPane({
  settings,
  activeProviderOptions,
  onSelectProvider,
  onChangeHotkey,
  hotkeyWarning = null,
  hasError = false,
}: SettingsPaneProps) {
  return (
    <div id="pane-settings" className="space-y-5">
      <PaneHeader
        title="Global"
        description={
          <>
            Select your active provider.
            <br />
            Use the default shortcut, or set up your own, to start a transcription.
          </>
        }
      />
      <div className="space-y-3 pt-1">
        <SectionLabel>Active Provider</SectionLabel>
        <div
          id="active-provider-list"
          role="radiogroup"
          aria-label="Active Provider"
          className="flex flex-col space-y-2"
        >
          {activeProviderOptions.map((provider) => {
            const isSelected = settings.active_provider === provider.id;
            return (
              <RadioOptionRow
                key={provider.id}
                as="button"
                role="radio"
                ariaChecked={isSelected}
                isSelected={isSelected}
                onClick={() => onSelectProvider(provider.id)}
                className="active-provider-option text-left"
              >
                <span>{provider.label}</span>
              </RadioOptionRow>
            );
          })}
        </div>
      </div>
      <div className="pt-2">
        <HotkeyRecorder
          label="Shortcut"
          value={settings.hotkey}
          onChange={onChangeHotkey}
          externalWarning={hotkeyWarning}
          hideWarning={hasError}
        />
      </div>
    </div>
  );
}
