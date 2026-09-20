import { FilePathInput } from "../../components/FilePathInput";
import { PaneHeader } from "../../components/PaneHeader";
import { RadioOptionRow } from "../../components/RadioOptionRow";
import { SectionLabel } from "../../components/SectionLabel";
import { LOCAL_MODEL_SLOTS, type LocalModel } from "../../lib/constants";
import type { DownloadUiState, LocalModelInfo } from "../../lib/types";
import { LocalModelRow } from "../components/LocalModelRow";

export interface LocalPaneProps {
  localInfos: LocalModelInfo[];
  isLocalModelSelected: (model: LocalModel) => boolean;
  isCustomLocal: boolean;
  downloadState: DownloadUiState;
  customPathInput: string;
  onSelectStandardModel: (model: LocalModel) => void;
  onSelectCustomModel: () => void;
  onChangeCustomPath: (val: string) => void;
  onSaveCustomPath: () => void;
  onBrowseCustomPath: () => void;
  onStartDownload: (downloadPath: string) => void;
  onCancelDownload: (downloadPath: string) => void;
  onOpenDeleteModal: (downloadPath: string, trigger: HTMLButtonElement) => void;
}

export function LocalPane({
  localInfos,
  isLocalModelSelected,
  isCustomLocal,
  downloadState,
  customPathInput,
  onSelectStandardModel,
  onSelectCustomModel,
  onChangeCustomPath,
  onSaveCustomPath,
  onBrowseCustomPath,
  onStartDownload,
  onCancelDownload,
  onOpenDeleteModal,
}: LocalPaneProps) {
  return (
    <div id="pane-local" className="space-y-5">
      <PaneHeader
        title="Local Provider"
        description={
          <>
            Manage your Whisper.cpp local models.
            <br />
            For additional models, download the files and set the model path.
          </>
        }
      />
      <div className="space-y-3 pt-1">
        <SectionLabel>Models</SectionLabel>
        <div className="flex flex-col space-y-2" id="local-model-list">
          {LOCAL_MODEL_SLOTS.map((model, slot) => {
            if (!model) {
              return (
                <div
                  key={`local-empty-${slot}`}
                  aria-hidden="true"
                  className="h-8 px-1 py-0"
                />
              );
            }

            const info = localInfos.find(
              (candidate) => candidate.download_path === model.downloadPath
            );
            const isSelected = isLocalModelSelected(model);

            return (
              <LocalModelRow
                key={model.downloadPath}
                model={model}
                isSelected={isSelected}
                info={info}
                downloadState={downloadState}
                onSelect={(m) => onSelectStandardModel(m)}
                onStartDownload={onStartDownload}
                onCancelDownload={onCancelDownload}
                onOpenDeleteModal={onOpenDeleteModal}
              />
            );
          })}

          <RadioOptionRow
            isSelected={isCustomLocal}
            onClick={() => void onSelectCustomModel()}
            className="local-model-option"
          >
            <span className="block min-w-0 flex-1 truncate">Custom Path</span>
          </RadioOptionRow>
        </div>
      </div>

      <div className="space-y-3 pt-2">
        <SectionLabel>Model Path</SectionLabel>
        <FilePathInput
          value={customPathInput}
          onChange={(event) => onChangeCustomPath(event.target.value)}
          onBlur={onSaveCustomPath}
          onBrowse={onBrowseCustomPath}
        />
      </div>
    </div>
  );
}
