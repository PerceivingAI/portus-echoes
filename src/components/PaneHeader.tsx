import React from "react";

export interface PaneHeaderProps {
  title: string;
  description: React.ReactNode;
  action?: React.ReactNode;
}

export function PaneHeader({ title, description, action }: PaneHeaderProps) {
  if (action) {
    return (
      <div className="flex items-center justify-between pb-1">
        <div>
          <h2 className="text-sm font-semibold text-slate-200 tracking-wide">
            {title}
          </h2>
          <p className="text-xs text-slate-400 mt-0.5">{description}</p>
        </div>
        {action}
      </div>
    );
  }

  return (
    <div>
      <h2 className="text-sm font-semibold text-slate-200 tracking-wide">
        {title}
      </h2>
      <p className="text-xs text-slate-400 mt-0.5">{description}</p>
    </div>
  );
}
