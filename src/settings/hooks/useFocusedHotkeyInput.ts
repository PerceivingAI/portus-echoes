import { useEffect } from "react";
import {
  resetFocusedHotkeyInput,
  sendFocusedHotkeyKeyEvent,
  setHotkeyCaptureActive,
} from "../../lib/api";

const HOTKEY_RECORDER_SELECTOR = "[data-hotkey-recorder]";

export function useFocusedHotkeyInput() {
  useEffect(() => {
    let captureActive = false;
    const updateCapture = (active: boolean) => {
      if (captureActive === active) return;
      captureActive = active;
      void setHotkeyCaptureActive(active).catch(() => {});
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      void sendFocusedHotkeyKeyEvent({
        key: event.key,
        ctrlKey: event.ctrlKey,
        altKey: event.altKey,
        shiftKey: event.shiftKey,
        metaKey: event.metaKey,
        pressed: true,
        repeat: event.repeat,
      }).catch(() => {});
    };
    const handleKeyUp = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      void sendFocusedHotkeyKeyEvent({
        key: event.key,
        ctrlKey: event.ctrlKey,
        altKey: event.altKey,
        shiftKey: event.shiftKey,
        metaKey: event.metaKey,
        pressed: false,
        repeat: false,
      }).catch(() => {});
    };
    const handleFocusIn = (event: FocusEvent) => {
      updateCapture(
        event.target instanceof Element &&
          event.target.matches(HOTKEY_RECORDER_SELECTOR)
      );
    };
    const handleFocusOut = (event: FocusEvent) => {
      if (
        event.target instanceof Element &&
        event.target.matches(HOTKEY_RECORDER_SELECTOR)
      ) {
        updateCapture(false);
      }
    };
    const handleWindowFocus = () => {
      updateCapture(
        document.activeElement instanceof Element &&
          document.activeElement.matches(HOTKEY_RECORDER_SELECTOR)
      );
    };
    const handleWindowBlur = () => {
      updateCapture(false);
      void resetFocusedHotkeyInput().catch(() => {});
    };

    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("keyup", handleKeyUp);
    window.addEventListener("focusin", handleFocusIn);
    window.addEventListener("focusout", handleFocusOut);
    window.addEventListener("focus", handleWindowFocus);
    window.addEventListener("blur", handleWindowBlur);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("keyup", handleKeyUp);
      window.removeEventListener("focusin", handleFocusIn);
      window.removeEventListener("focusout", handleFocusOut);
      window.removeEventListener("focus", handleWindowFocus);
      window.removeEventListener("blur", handleWindowBlur);
      if (captureActive) void setHotkeyCaptureActive(false).catch(() => {});
      void resetFocusedHotkeyInput().catch(() => {});
    };
  }, []);
}
