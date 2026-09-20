import { getCurrentWindow } from "@tauri-apps/api/window";
import { X } from "lucide-react";

/** Step 1 — Welcome Screen (Header, Body, Bottom Area). */
export function StepWelcome({ onNext }: { onNext: () => void }) {
  const handleDrag = (e: React.MouseEvent) => {
    if (e.button === 0) {
      const target = e.target as HTMLElement;
      if (target.closest("button") || target.closest("input")) {
        return;
      }
      e.preventDefault();
      e.stopPropagation();
      getCurrentWindow().startDragging().catch(() => {});
    }
  };

  return (
    <div
      data-tauri-drag-region
      onMouseDown={handleDrag}
      onDoubleClick={(e) => e.preventDefault()}
      className="flex h-full flex-col justify-between relative overflow-hidden select-none cursor-move bg-hud-panel text-slate-100"
    >
      {/* 1. HEADER AREA */}
      <header
        data-tauri-drag-region
        className="flex-none pl-6 pr-8 py-4 flex items-center justify-between cursor-move"
      >
        <h1
          data-tauri-drag-region
          className="text-xl font-semibold tracking-tight text-slate-100 cursor-move select-none"
        >
          <span className="text-accent" data-tauri-drag-region>Portus</span>Echoes
        </h1>
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            getCurrentWindow().hide();
          }}
          onMouseDown={(e) => e.stopPropagation()}
          className="flex h-7 w-7 items-center justify-center rounded-md text-slate-400 hover:text-slate-200 transition-colors focus:outline-none cursor-pointer -mr-[2px]"
          aria-label="Close"
        >
          <X className="w-4 h-4" />
        </button>
      </header>

      {/* 2. BODY & FOOTER GROUP */}
      <div className="flex-1 flex flex-col justify-between">
        <main
          data-tauri-drag-region
          className="flex-1 flex flex-col items-center justify-center text-center p-6 pb-2 cursor-move"
        >
          <h2
            data-tauri-drag-region
            className="text-2xl font-semibold tracking-tight text-slate-200 cursor-move"
          >
            Welcome!
          </h2>
          <p
            data-tauri-drag-region
            className="mt-5 text-sm text-slate-400 max-w-sm cursor-move"
          >
            Configure your cloud or local transcription providers.
            <br />
            Set up a shortcut for ease of use.
          </p>
        </main>

        <footer
          data-tauri-drag-region
          className="flex-none px-6 pt-0 pb-28 flex items-center justify-center cursor-move"
        >
          <button
            type="button"
            onClick={onNext}
            onMouseDown={(e) => e.stopPropagation()}
            className="px-8 py-2.5 bg-slate-200 hover:bg-slate-300 text-slate-950 text-sm font-semibold rounded-lg transition-colors duration-200 shadow-accent-button tracking-wide cursor-pointer"
          >
            Continue
          </button>
        </footer>
      </div>
    </div>
  );
}
