import React, { useEffect, useState } from "react";
import { isHotkeyRegistered } from "../lib/api";
import { SectionLabel } from "./SectionLabel";

const KEY_NAMES: Record<string, string> = {
  " ": "Space",
  Spacebar: "Space",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Escape: "Esc",
};

function comboFromEvent(e: KeyboardEvent): string | null {
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Super");
  // Ignore pure modifier presses — the combo needs a non-modifier key.
  if (["Control", "Alt", "Shift", "Meta"].includes(e.key)) return null;
  const key = KEY_NAMES[e.key] ?? (e.key.length === 1 ? e.key.toUpperCase() : e.key);
  parts.push(key);
  return parts.join("+");
}

/**
 * Key-capture input (docs/GUI.md). Every shortcut advisory uses the same
 * reserved inline warning position below this input.
 */
export function HotkeyRecorder({
  value,
  onChange,
  placeholder,
  externalWarning = null,
  hideWarning = false,
  label,
}: {
  value: string;
  onChange: (combo: string) => void;
  placeholder?: string;
  externalWarning?: string | null;
  hideWarning?: boolean;
  label?: React.ReactNode;
}) {
  const [capturing, setCapturing] = useState(false);
  const [conflict, setConflict] = useState(false);
  const noModifier = value !== "" && !value.includes("+");
  const warning =
    externalWarning ??
    (conflict
      ? "Combination registered by another app."
      : noModifier
        ? "Key without modifiers may interfere with typing."
        : null);
  useEffect(() => {
    let active = true;
    if (value === "" || noModifier) {
      setConflict(false);
      return () => {
        active = false;
      };
    }
    const handle = setTimeout(() => {
      void isHotkeyRegistered(value)
        .then((registered) => {
          if (active) setConflict(registered);
        })
        .catch(() => {
          if (active) setConflict(false);
        });
    }, 250);
    return () => {
      active = false;
      clearTimeout(handle);
    };
  }, [value, noModifier]);

  return (
    <div className="relative w-full space-y-3">
      {label ? (
        <div className="flex items-center justify-between">
          {typeof label === "string" ? <SectionLabel>{label}</SectionLabel> : label}
          {!hideWarning && warning ? (
            <p className="text-xs text-amber-400 text-center leading-tight">
              ⚠ {warning}
            </p>
          ) : null}
        </div>
      ) : null}
      <div className="relative flex w-full items-center border-b border-hud-border pb-1 transition-colors duration-200 focus-within:border-slate-200 -translate-y-[3px]">
        <input
          type="text"
          readOnly
          data-hotkey-recorder
          value={capturing ? "Press key..." : value}
          placeholder={placeholder}
          onFocus={() => setCapturing(true)}
          onBlur={() => setCapturing(false)}
          onKeyDown={(e) => {
            e.preventDefault();
            const combo = comboFromEvent(e.nativeEvent);
            if (combo) {
              onChange(combo);
              setCapturing(false);
              e.currentTarget.blur();
            }
          }}
          className="custom-input w-full text-center truncate bg-transparent py-2 text-[15px] font-medium tracking-wide text-slate-100 placeholder-slate-500 outline-none cursor-pointer focus:border-slate-200"
        />
      </div>
      {!label && !hideWarning && warning ? (
        <p className="text-xs text-amber-400 text-center leading-tight">
          ⚠ {warning}
        </p>
      ) : null}
    </div>
  );
}
