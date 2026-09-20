import React from "react";

export interface RadioOptionRowProps {
  isSelected: boolean;
  onClick: (event: React.MouseEvent<HTMLElement>) => void;
  className?: string;
  minHeightClass?: string;
  as?: "div" | "button";
  role?: string;
  ariaChecked?: boolean;
  children: React.ReactNode;
}

export function RadioOptionRow({
  isSelected,
  onClick,
  className = "",
  minHeightClass = "h-8",
  as: Component = "div",
  role,
  ariaChecked,
  children,
}: RadioOptionRowProps) {
  const baseClasses = `group flex ${minHeightClass} cursor-pointer items-center gap-3 px-1 py-0 text-sm font-semibold transition-colors duration-200 active:opacity-80 ${
    isSelected ? "text-slate-100" : "text-slate-400 hover:text-slate-100"
  } ${className}`.trim();

  return (
    <Component
      type={Component === "button" ? "button" : undefined}
      role={role}
      aria-checked={ariaChecked}
      onClick={onClick}
      className={baseClasses}
    >
      <span
        aria-hidden="true"
        className={`h-2.5 w-2.5 shrink-0 rounded-full transition-colors ${
          isSelected
            ? "bg-accent shadow-accent-dot"
            : "border border-slate-500/60 bg-transparent"
        }`}
      />
      {children}
    </Component>
  );
}
