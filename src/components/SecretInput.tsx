import React from "react";
import { Eye, EyeOff } from "lucide-react";

export interface SecretInputProps {
  value: string;
  showKey: boolean;
  placeholder?: string;
  onChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  onBlur: () => void;
  onKeyDown?: (event: React.KeyboardEvent<HTMLInputElement>) => void;
  onToggleShow: () => void;
  ariaLabel?: string;
}

export function SecretInput({
  value,
  showKey,
  placeholder = "Your API Key",
  onChange,
  onBlur,
  onKeyDown,
  onToggleShow,
  ariaLabel = "Toggle API Key visibility",
}: SecretInputProps) {
  return (
    <div className="relative flex items-center border-b border-hud-border pb-1 transition-colors duration-200 focus-within:border-slate-200">
      <input
        type={showKey ? "text" : "password"}
        value={value}
        onChange={onChange}
        onBlur={onBlur}
        onKeyDown={onKeyDown}
        placeholder={placeholder}
        className="custom-input w-full bg-transparent py-2 pr-10 text-sm font-semibold text-slate-100 placeholder:text-sm placeholder:font-semibold outline-none focus:border-slate-200"
      />
      <button
        type="button"
        onClick={onToggleShow}
        className="absolute right-2 rounded-md p-1 text-slate-400 transition-colors hover:text-slate-200 focus:outline-none"
        aria-label={ariaLabel}
      >
        {showKey ? (
          <EyeOff className="w-4 h-4" />
        ) : (
          <Eye className="w-4 h-4" />
        )}
      </button>
    </div>
  );
}
