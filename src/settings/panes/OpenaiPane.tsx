import { PaneHeader } from "../../components/PaneHeader";
import { RadioOptionRow } from "../../components/RadioOptionRow";
import { SecretInput } from "../../components/SecretInput";
import { SectionLabel } from "../../components/SectionLabel";
import { OPENAI_MODEL_SLOTS } from "../../lib/constants";
import { CustomCloudModelRow } from "../components/CustomCloudModelRow";

export interface CloudSelection {
  kind: "standard" | "custom";
  id?: string;
}

export interface OpenaiPaneProps {
  selection: CloudSelection | null;
  customModelInput: string;
  apiKey: string;
  showKey: boolean;
  onSelectStandardModel: (modelId: string) => void;
  onSelectCustomModel: () => void;
  onChangeCustomModel: (val: string) => void;
  onSaveCustomModel: () => void;
  onChangeKey: (val: string) => void;
  onSaveKey: () => void;
  onToggleShowKey: () => void;
}

export function OpenaiPane({
  selection,
  customModelInput,
  apiKey,
  showKey,
  onSelectStandardModel,
  onSelectCustomModel,
  onChangeCustomModel,
  onSaveCustomModel,
  onChangeKey,
  onSaveKey,
  onToggleShowKey,
}: OpenaiPaneProps) {
  return (
    <div id="pane-openai" className="space-y-5">
      <PaneHeader
        title="Cloud Provider"
        description={
          <>
            Select an OpenAI model and add your API Key.
            <br />
            For additional OpenAI models, add a Custom Model ID.
          </>
        }
      />
      <div className="space-y-3 pt-1">
        <SectionLabel>Models</SectionLabel>
        <div className="flex flex-col space-y-2" id="openai-model-list">
          {OPENAI_MODEL_SLOTS.map((model, slot) => {
            if (!model) {
              return (
                <div
                  key={`openai-empty-${slot}`}
                  aria-hidden="true"
                  className="h-8 px-1 py-0"
                />
              );
            }
            const isSelected =
              selection?.kind === "standard" && selection.id === model.id;
            return (
              <RadioOptionRow
                key={`${model.id}-${slot}`}
                isSelected={isSelected}
                onClick={() => onSelectStandardModel(model.id)}
                className="cloud-model-option"
              >
                <span className="block line-clamp-2" title={model.label}>
                  {model.label}
                </span>
              </RadioOptionRow>
            );
          })}

          <CustomCloudModelRow
            isSelected={selection?.kind === "custom"}
            value={customModelInput}
            onSelect={onSelectCustomModel}
            onChange={(event) => onChangeCustomModel(event.target.value)}
            onBlur={onSaveCustomModel}
          />
        </div>
      </div>

      <div className="space-y-3 pt-2">
        <SectionLabel>API Key</SectionLabel>
        <SecretInput
          value={apiKey}
          showKey={showKey}
          onChange={(event) => onChangeKey(event.target.value)}
          onBlur={onSaveKey}
          onKeyDown={(e) => e.key === "Enter" && onSaveKey()}
          onToggleShow={onToggleShowKey}
        />
      </div>
    </div>
  );
}
