import React from "react";
import { X } from "lucide-react";

export interface SettingsHeaderProps {
  onClose: () => void;
  onDrag: (event: React.MouseEvent) => void;
}

export function SettingsHeader({ onClose, onDrag }: SettingsHeaderProps) {
  return (
    <header
      data-tauri-drag-region
      onMouseDown={onDrag}
      className="flex-none pl-6 pr-8 py-4 flex items-center justify-between cursor-move bg-hud-panel border-b border-hud-border"
    >
      <h1
        data-tauri-drag-region
        className="text-xl font-semibold tracking-tight text-slate-100 cursor-move select-none"
      >
        <span className="text-accent" data-tauri-drag-region>Portus</span>Echoes
      </h1>
      <button
        type="button"
        onClick={(event) => {
          event.stopPropagation();
          onClose();
        }}
        onMouseDown={(e) => e.stopPropagation()}
        className="flex h-7 w-7 items-center justify-center rounded-md text-slate-400 hover:text-slate-200 transition-colors focus:outline-none cursor-pointer -mr-[2px]"
        aria-label="Close"
      >
        <X className="w-4 h-4" />
      </button>
    </header>
  );
}
