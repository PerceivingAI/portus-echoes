import React from "react";
import type { LucideIcon } from "lucide-react";

export interface NavigationItem<TabId extends string = string> {
  id: TabId;
  label: string;
  Icon: LucideIcon;
}

export interface SettingsNavProps<TabId extends string = string> {
  activeTab: TabId;
  navigationItems: ReadonlyArray<NavigationItem<TabId>>;
  onSelectTab: (tab: TabId) => void;
  onboardingComplete: boolean;
  isAnyConfigured: boolean;
  busy: boolean;
  onFinish: () => void;
}

export function SettingsNav<TabId extends string = string>({
  activeTab,
  navigationItems,
  onSelectTab,
  onboardingComplete,
  isAnyConfigured,
  busy,
  onFinish,
}: SettingsNavProps<TabId>) {
  return (
    <aside
      aria-label="Settings navigation panel"
      className="flex w-40 shrink-0 flex-col border-r border-hud-border bg-hud-panel px-3 pt-3 pb-1"
    >
      <nav aria-label="Settings sections" className="space-y-1.5">
        {navigationItems.map(({ id, label, Icon }) => {
          const isActive = activeTab === id;
          return (
            <button
              key={id}
              type="button"
              onClick={() => onSelectTab(id)}
              onMouseDown={(event) => event.stopPropagation()}
              style={{ WebkitAppRegion: "no-drag" } as React.CSSProperties}
              aria-current={isActive ? "page" : undefined}
              className={`group flex w-full items-center gap-3 rounded-lg border px-3 py-2.5 text-left text-sm font-medium tracking-wide transition-all duration-200 active:scale-[0.98] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent/70 ${
                isActive
                  ? "active border-transparent bg-hud-navActive/50 text-slate-100"
                  : "border-transparent text-slate-400 hover:border-hud-border/70 hover:bg-hud-cardHover/70 hover:text-slate-200"
              }`}
            >
              <Icon
                aria-hidden="true"
                className={`h-4 w-4 shrink-0 ${
                  isActive
                    ? "text-slate-100"
                    : "text-slate-400 group-hover:text-slate-200"
                }`}
              />
              <span>{label}</span>
            </button>
          );
        })}
      </nav>

      <div className="mt-auto pt-4">
        {!onboardingComplete && (
          <button
            type="button"
            onMouseDown={(event) => event.stopPropagation()}
            style={{ WebkitAppRegion: "no-drag" } as React.CSSProperties}
            onClick={() => void onFinish()}
            disabled={!isAnyConfigured || busy}
            className="w-full px-4 py-2.5 bg-slate-200 hover:bg-slate-300 text-slate-950 text-sm font-semibold rounded-lg transition-colors duration-200 shadow-accent-button tracking-wide disabled:opacity-40 disabled:cursor-not-allowed cursor-pointer focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent/70"
          >
            {busy ? "Starting…" : "Start"}
          </button>
        )}
        <div className="mt-2 flex h-8 shrink-0 items-start justify-center">
          {!isAnyConfigured && (
            <p className="text-center text-[11px] leading-tight text-slate-400">
              Configure at least one provider.
            </p>
          )}
        </div>
      </div>
    </aside>
  );
}
