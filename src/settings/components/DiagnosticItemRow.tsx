export interface DiagnosticItemRowProps {
  message: string;
}

export function DiagnosticItemRow({ message }: DiagnosticItemRowProps) {
  return (
    <div className="py-0.5 min-w-0" data-diagnostic-item-row>
      <p
        className="text-xs text-slate-400 whitespace-nowrap truncate min-w-0"
        title={message}
      >
        {message}
      </p>
    </div>
  );
}
