import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type MutableRefObject,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  isHotkeyActive,
  selectActiveProvider,
  setHotkey,
} from "../../lib/api";
import { errorMessageForCode } from "../../lib/errors";
import { isProviderId, type AppSettings, type ProviderId } from "../../lib/types";
import { useTauriEvent } from "../../hooks/useTauriEvent";
import { enqueue } from "./queue";

type ApplySettings = (patch: Partial<AppSettings>) => void;
type SetError = (message: string | null) => void;

const HOTKEY_SAVE_WARNING = "Your shortcut could not be saved.";
const HOTKEY_UNAVAILABLE_WARNING = "Global shortcuts are unavailable on this system.";
const HOTKEY_TRANSIENT_WARNING_MS = 1500;

export function useSettingsLifecycle({
  settingsRef,
  applySettings,
  setError,
  flushDirtyFields,
  onFinish,
}: {
  settingsRef: MutableRefObject<AppSettings>;
  applySettings: ApplySettings;
  setError: SetError;
  flushDirtyFields: () => Promise<void>;
  onFinish: () => Promise<void>;
}) {
  const providerQueue = useRef<Promise<void>>(Promise.resolve());
  const hotkeyQueue = useRef<Promise<void>>(Promise.resolve());
  const providerEventRevisionRef = useRef(0);
  const hotkeyRequestRef = useRef(0);
  const persistedHotkeyRef = useRef(settingsRef.current.hotkey);
  const busyRef = useRef(false);
  const hotkeyWarningTimerRef = useRef<number | null>(null);
  const hotkeyActiveRequestRef = useRef(0);
  const [busy, setBusy] = useState(false);
  const [hotkeyActive, setHotkeyActive] = useState<boolean | null>(null);
  const [hotkeySaveWarning, setHotkeySaveWarning] = useState(false);

  const refreshHotkeyActive = useCallback(() => {
    const request = ++hotkeyActiveRequestRef.current;
    void isHotkeyActive()
      .then((registered) => {
        if (hotkeyActiveRequestRef.current === request) {
          setHotkeyActive(registered);
        }
      })
      .catch(() => {});
  }, []);

  useEffect(() => {
    refreshHotkeyActive();
    return () => {
      hotkeyActiveRequestRef.current += 1;
      if (hotkeyWarningTimerRef.current !== null) {
        window.clearTimeout(hotkeyWarningTimerRef.current);
        hotkeyWarningTimerRef.current = null;
      }
    };
  }, [refreshHotkeyActive]);

  const clearHotkeySaveWarning = useCallback(() => {
    if (hotkeyWarningTimerRef.current !== null) {
      window.clearTimeout(hotkeyWarningTimerRef.current);
      hotkeyWarningTimerRef.current = null;
    }
    setHotkeySaveWarning(false);
  }, []);

  const showHotkeySaveWarning = useCallback(() => {
    if (hotkeyWarningTimerRef.current !== null) {
      window.clearTimeout(hotkeyWarningTimerRef.current);
    }
    setHotkeySaveWarning(true);
    hotkeyWarningTimerRef.current = window.setTimeout(() => {
      setHotkeySaveWarning(false);
      hotkeyWarningTimerRef.current = null;
    }, HOTKEY_TRANSIENT_WARNING_MS);
  }, []);

  useTauriEvent<unknown>("settings:active-provider-changed", (provider) => {
    if (!isProviderId(provider)) return;
    providerEventRevisionRef.current += 1;
    if (settingsRef.current.active_provider === provider) return;
    applySettings({ active_provider: provider });
  });

  const selectProvider = useCallback(
    (provider: ProviderId) => {
      setError(null);
      const operation = enqueue(providerQueue, async () => {
        const eventRevision = providerEventRevisionRef.current;
        const next = await selectActiveProvider(provider);
        if (
          providerEventRevisionRef.current === eventRevision &&
          settingsRef.current.active_provider !== next.active_provider
        ) {
          applySettings({ active_provider: next.active_provider });
        }
      });
      void operation.catch((cause) => setError(errorMessageForCode(cause)));
    },
    [applySettings, setError, settingsRef]
  );

  const changeHotkey = useCallback(
    (combo: string) => {
      const request = ++hotkeyRequestRef.current;
      setError(null);
      clearHotkeySaveWarning();
      const operation = enqueue(hotkeyQueue, async () => {
        try {
          const next = await setHotkey(combo);
          persistedHotkeyRef.current = next.hotkey;
          if (hotkeyRequestRef.current === request) {
            applySettings({ hotkey: next.hotkey });
            refreshHotkeyActive();
          }
        } catch {
          if (hotkeyRequestRef.current === request) {
            applySettings({ hotkey: persistedHotkeyRef.current });
            refreshHotkeyActive();
            showHotkeySaveWarning();
          }
        }
      });
      void operation;
    },
    [
      applySettings,
      clearHotkeySaveWarning,
      refreshHotkeyActive,
      setError,
      showHotkeySaveWarning,
    ]
  );

  const handleFinish = useCallback(async () => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError(null);
    try {
      await Promise.all([
        flushDirtyFields(),
        providerQueue.current,
        hotkeyQueue.current,
      ]);
      if (!settingsRef.current.onboarding_complete) {
        await onFinish();
      }
      await getCurrentWindow().hide();
    } catch (cause) {
      setError(errorMessageForCode(cause));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }, [flushDirtyFields, onFinish, setError, settingsRef]);

  useTauriEvent("settings:close-requested", () => {
    void handleFinish();
  });

  const hotkeyWarning = hotkeySaveWarning
    ? HOTKEY_SAVE_WARNING
    : hotkeyActive === false
      ? HOTKEY_UNAVAILABLE_WARNING
      : null;

  return {
    busy,
    selectProvider,
    changeHotkey,
    handleFinish,
    hotkeyWarning,
  };
}
