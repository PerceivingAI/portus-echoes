import React from "react";
import { Folder } from "lucide-react";

export interface FilePathInputProps {
  value: string;
  placeholder?: string;
  onChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  onBlur: () => void;
  onBrowse: () => void;
  buttonAriaLabel?: string;
}

export function FilePathInput({
  value,
  placeholder = "Custom path to a local model file (.bin)",
  onChange,
  onBlur,
  onBrowse,
  buttonAriaLabel = "Browse model file",
}: FilePathInputProps) {
  return (
    <div className="relative flex items-center border-b border-hud-border pb-1 transition-colors duration-200 focus-within:border-slate-200">
      <input
        type="text"
        value={value}
        onChange={onChange}
        onBlur={onBlur}
        placeholder={placeholder}
        className="custom-input w-full truncate bg-transparent py-2 pr-10 text-sm font-semibold text-slate-100 placeholder:text-sm placeholder:font-semibold outline-none cursor-text focus:border-slate-200"
      />
      <button
        type="button"
        onClick={onBrowse}
        className="absolute right-2 rounded-md p-1 text-slate-400 transition-colors hover:text-slate-200 focus:outline-none"
        aria-label={buttonAriaLabel}
      >
        <Folder className="w-4 h-4" />
      </button>
    </div>
  );
}
