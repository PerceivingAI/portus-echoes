import React from "react";

export interface ConfirmModalProps {
  isOpen: boolean;
  message: string;
  messageId?: string;
  confirmText?: string;
  cancelText?: string;
  isPending?: boolean;
  cancelRef?: React.RefObject<HTMLButtonElement>;
  confirmRef?: React.RefObject<HTMLButtonElement>;
  onConfirm: () => void;
  onCancel: () => void;
  onKeyDown?: (event: React.KeyboardEvent<HTMLDivElement>) => void;
}

export function ConfirmModal({
  isOpen,
  message,
  messageId = "delete-model-message",
  confirmText = "Confirm",
  cancelText = "Cancel",
  isPending = false,
  cancelRef,
  confirmRef,
  onConfirm,
  onCancel,
  onKeyDown,
}: ConfirmModalProps) {
  if (!isOpen) return null;

  return (
    <div
      className="absolute inset-0 z-50 flex items-center justify-center bg-black/70 p-6"
      style={{ WebkitAppRegion: "no-drag" } as React.CSSProperties}
      onMouseDown={(event) => event.stopPropagation()}
      onClick={(event) => {
        if (event.target === event.currentTarget) {
          onCancel();
        }
      }}
      onKeyDown={onKeyDown}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={messageId}
        className="w-full max-w-sm rounded-xl bg-hud-panel p-6 shadow-modal-overlay"
        onClick={(event) => event.stopPropagation()}
      >
        <p id={messageId} className="text-sm font-semibold text-slate-200 tracking-wide">
          {message}
        </p>
        <div className="mt-6 flex items-center justify-end gap-3">
          <button
            ref={cancelRef}
            type="button"
            onClick={onCancel}
            className="px-7 py-2.5 bg-transparent text-slate-200 text-sm font-semibold rounded-lg transition-all duration-200 hover:bg-white/5 tracking-wide cursor-pointer"
          >
            {cancelText}
          </button>
          <button
            ref={confirmRef}
            type="button"
            onClick={onConfirm}
            disabled={isPending}
            className="px-7 py-2.5 bg-slate-200 hover:bg-slate-300 text-slate-950 text-sm font-semibold rounded-lg transition-colors duration-200 shadow-accent-button tracking-wide disabled:opacity-40 disabled:cursor-not-allowed cursor-pointer"
          >
            {confirmText}
          </button>
        </div>
      </div>
    </div>
  );
}
