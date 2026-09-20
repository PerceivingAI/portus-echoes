import { Check, Download, Trash2, X } from "lucide-react";
import { RadioOptionRow } from "../../components/RadioOptionRow";
import type { LocalModel } from "../../lib/constants";
import type { DownloadUiState, LocalModelInfo } from "../../lib/types";

export interface LocalModelRowProps {
  model: LocalModel;
  isSelected: boolean;
  info?: LocalModelInfo;
  downloadState: DownloadUiState;
  onSelect: (model: LocalModel) => void;
  onStartDownload: (downloadPath: string) => void;
  onCancelDownload: (downloadPath: string) => void;
  onOpenDeleteModal: (downloadPath: string, trigger: HTMLButtonElement) => void;
}

export function LocalModelRow({
  model,
  isSelected,
  info,
  downloadState,
  onSelect,
  onStartDownload,
  onCancelDownload,
  onOpenDeleteModal,
}: LocalModelRowProps) {
  const isDownloading =
    downloadState.phase === "downloading" &&
    downloadState.downloadPath === model.downloadPath;
  const isSuccess =
    downloadState.phase === "success" &&
    downloadState.downloadPath === model.downloadPath;
  const isReady = info?.downloaded === true || isSuccess;
  const totalBytes = isDownloading ? downloadState.totalBytes : null;
  const hasProgress = isDownloading && downloadState.progressReceived;
  const hasKnownTotal =
    hasProgress && totalBytes !== null && totalBytes > 0;
  const progressPercent = hasKnownTotal
    ? Math.min(
        100,
        Math.max(0, (downloadState.downloadedBytes / totalBytes) * 100)
      )
    : 0;

  return (
    <RadioOptionRow
      isSelected={isSelected}
      onClick={() => onSelect(model)}
      className="local-model-option"
    >
      <span className="block min-w-0 flex-1 truncate" title={model.label}>
        {model.label}
      </span>
      <div className="ml-auto flex w-2/5 min-w-0 shrink-0 items-center justify-end gap-3">
        <div className="min-w-0 flex-1">
          {isDownloading ? (
            <div
              role="progressbar"
              aria-label={`Downloading ${model.label}`}
              aria-valuemin={hasKnownTotal ? 0 : undefined}
              aria-valuemax={hasKnownTotal ? totalBytes : undefined}
              aria-valuenow={
                hasKnownTotal ? downloadState.downloadedBytes : undefined
              }
              className="h-1 w-full overflow-hidden rounded-full bg-slate-700/60"
            >
              <div
                className={`h-full rounded-full bg-slate-200 ${
                  hasKnownTotal
                    ? "transition-[width] duration-150 ease-linear"
                    : downloadState.progressReceived
                      ? "w-1/3 animate-pulse"
                      : "w-0"
                }`}
                style={
                  hasKnownTotal ? { width: `${progressPercent}%` } : undefined
                }
              />
            </div>
          ) : isReady ? (
            <span className="block text-right text-xs text-slate-400">
              Ready
            </span>
          ) : null}
        </div>
        <div className="flex h-6 w-6 shrink-0 items-center justify-center mr-1">
          {isDownloading ? (
            <button
              type="button"
              onClick={(event) => {
                event.stopPropagation();
                onCancelDownload(model.downloadPath);
              }}
              disabled={downloadState.cancelRequested}
              className="text-slate-400 hover:text-slate-200 transition focus:outline-none p-1 rounded-md disabled:opacity-50 disabled:cursor-not-allowed"
              aria-label="Cancel model download"
            >
              <X className="w-4 h-4" />
            </button>
          ) : isSuccess ? (
            <Check
              role="img"
              aria-label="Download complete"
              className="w-4 h-4 text-accent"
            />
          ) : info?.downloaded ? (
            <button
              type="button"
              onClick={(event) => {
                event.stopPropagation();
                onOpenDeleteModal(
                  model.downloadPath,
                  event.currentTarget
                );
              }}
              className="text-slate-400 hover:text-slate-200 transition focus:outline-none p-1 rounded-md"
              aria-label="Delete model"
            >
              <Trash2 className="w-4 h-4" />
            </button>
          ) : (
            <button
              type="button"
              onClick={(event) => {
                event.stopPropagation();
                onStartDownload(model.downloadPath);
              }}
              disabled={downloadState.phase !== "idle"}
              className="text-slate-400 hover:text-slate-200 transition focus:outline-none p-1 rounded-md disabled:opacity-50 disabled:cursor-not-allowed"
              aria-label="Download model"
            >
              <Download className="w-4 h-4" />
            </button>
          )}
        </div>
      </div>
    </RadioOptionRow>
  );
}
