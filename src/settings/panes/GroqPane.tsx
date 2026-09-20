import { PaneHeader } from "../../components/PaneHeader";
import { RadioOptionRow } from "../../components/RadioOptionRow";
import { SecretInput } from "../../components/SecretInput";
import { SectionLabel } from "../../components/SectionLabel";
import { GROQ_MODEL_SLOTS } from "../../lib/constants";
import { CustomCloudModelRow } from "../components/CustomCloudModelRow";
import type { CloudSelection } from "./OpenaiPane";

export interface GroqPaneProps {
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

export function GroqPane({
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
}: GroqPaneProps) {
  return (
    <div id="pane-groq" className="space-y-5">
      <PaneHeader
        title="Cloud Provider"
        description={
          <>
            Select a Groq model and add your API Key.
            <br />
            For additional Groq models, add a Custom Model ID.
          </>
        }
      />
      <div className="space-y-3 pt-1">
        <SectionLabel>Models</SectionLabel>
        <div className="flex flex-col space-y-2" id="groq-model-list">
          {GROQ_MODEL_SLOTS.map((model, slot) => {
            if (!model) {
              return (
                <div
                  key={`groq-empty-${slot}`}
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
