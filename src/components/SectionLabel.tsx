import React from "react";

export interface SectionLabelProps {
  children: React.ReactNode;
  className?: string;
}

export function SectionLabel({ children, className = "" }: SectionLabelProps) {
  return (
    <p
      className={`text-xs font-semibold uppercase tracking-wider text-slate-400 ${className}`.trim()}
    >
      {children}
    </p>
  );
}
