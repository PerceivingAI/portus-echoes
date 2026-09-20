import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  completeOnboarding,
  getSettings,
  getSettingsNavigation,
} from "../lib/api";
import { DEFAULT_SETTINGS } from "../lib/constants";
import {
  isSettingsNavigationState,
  type AppSettings,
  type SettingsNavigationState,
  type UserErrorCode,
} from "../lib/types";
import { useTauriEvent } from "../hooks/useTauriEvent";
import { StepWelcome } from "./StepWelcome";
import { StepSetupTabs as Settings } from "../settings/Settings";

export function Onboarding() {
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [screen, setScreen] = useState<1 | 2 | null>(null);
  const [startupErrorCode, setStartupErrorCode] = useState<UserErrorCode | undefined>();
  const [navigation, setNavigation] = useState<SettingsNavigationState | null>(null);
  const navigationRevisionRef = useRef(0);
  const screenRef = useRef<typeof screen>(screen);
  screenRef.current = screen;

  const applyNavigation = useCallback((incoming: SettingsNavigationState) => {
    if (incoming.revision < navigationRevisionRef.current) return;
    navigationRevisionRef.current = incoming.revision;
    setNavigation(incoming);
    setScreen(2);
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void (async () => {
      let stopListening: () => void;
      try {
        stopListening = await listen<unknown>(
          "settings:navigation-requested",
          (event) => {
            if (!disposed && isSettingsNavigationState(event.payload)) {
              applyNavigation(event.payload);
            }
          }
        );
      } catch {
        return;
      }

      if (disposed) {
        stopListening();
        return;
      }
      unlisten = stopListening;

      try {
        const snapshot = await getSettingsNavigation();
        if (!disposed && isSettingsNavigationState(snapshot)) {
          applyNavigation(snapshot);
        }
      } catch {
        // Live navigation events remain authoritative if the snapshot read fails.
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [applyNavigation]);

  useTauriEvent("settings:close-requested", () => {
    if (screenRef.current !== 2) {
      void getCurrentWindow().hide();
    }
  });

  useEffect(() => {
    let active = true;
    getSettings()
      .then((loaded) => {
        if (!active) return;
        setSettings(loaded);
        setScreen(
          navigationRevisionRef.current > 0 || loaded.onboarding_complete ? 2 : 1
        );
      })
      .catch(() => {
        if (!active) return;
        setSettings(DEFAULT_SETTINGS);
        setStartupErrorCode("unexpected");
        setScreen(2);
      });

    return () => {
      active = false;
    };
  }, []);

  const finish = async () => {
    const final = await completeOnboarding();
    setSettings(final);
  };

  return (
    <div className="h-full w-full bg-hud-panel rounded-2xl overflow-hidden text-slate-100 select-none flex flex-col">
      {screen === 1 ? (
        <StepWelcome onNext={() => setScreen(2)} />
      ) : screen === 2 ? (
        <Settings
          settings={settings}
          onSaved={setSettings}
          onFinish={finish}
          initialTab={navigation?.tab ?? "settings"}
          navigationTab={navigation?.tab}
          initialErrorCode={startupErrorCode}
        />
      ) : null}
    </div>
  );
}
