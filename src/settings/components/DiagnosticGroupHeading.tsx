import { Check, X } from "lucide-react";
import type { DiagnosticGroupStatus } from "../../lib/types";

export interface DiagnosticGroupHeadingProps {
  title: string;
  status?: DiagnosticGroupStatus;
}

export function DiagnosticGroupHeading({
  title,
  status,
}: DiagnosticGroupHeadingProps) {
  return (
    <div
      className="flex items-center gap-2 mb-1 min-w-0"
      data-diagnostic-group-heading={title}
    >
      <h3 className="text-sm font-medium text-slate-200 truncate">{title}</h3>
      <span
        className={`w-4 h-4 flex items-center justify-center shrink-0 ${
          status === "pass"
            ? "text-accent"
            : status === "fail"
              ? "text-red-400"
              : ""
        }`}
        aria-hidden="true"
      >
        {status === "pass" ? (
          <Check className="w-4 h-4" data-diagnostic-group-icon="pass" />
        ) : status === "fail" ? (
          <X className="w-4 h-4" data-diagnostic-group-icon="fail" />
        ) : null}
      </span>
    </div>
  );
}
