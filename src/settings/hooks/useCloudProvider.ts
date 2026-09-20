import { useCallback, useEffect, useRef, useState } from "react";
import {
  deleteApiKey,
  getApiKey,
  selectGroqCustomModel,
  selectGroqModel,
  selectOpenaiCustomModel,
  selectOpenaiModel,
  setApiKey,
  setGroqCustomModel,
  setOpenaiCustomModel,
} from "../../lib/api";
import { errorMessageForCode } from "../../lib/errors";
import type { AppSettings } from "../../lib/types";
import { useDirtyText } from "../../hooks/useDirtyText";
import type { CloudSelection } from "../panes/OpenaiPane";
import { enqueue } from "./queue";

type CloudProviderId = "openai" | "groq";
type ApplySettings = (patch: Partial<AppSettings>) => void;
type SetError = (message: string | null) => void;

interface CloudProviderConfig {
  model: (settings: AppSettings) => string;
  modelKind: (settings: AppSettings) => AppSettings["openai_model_kind"];
  customModel: (settings: AppSettings) => string;
  selectStandard: (model: string) => Promise<AppSettings>;
  selectCustom: () => Promise<AppSettings>;
  setCustom: (model: string) => Promise<AppSettings>;
}

const CONFIG: Record<CloudProviderId, CloudProviderConfig> = {
  openai: {
    model: (settings) => settings.openai_model,
    modelKind: (settings) => settings.openai_model_kind,
    customModel: (settings) => settings.openai_custom_model,
    selectStandard: (model) => selectOpenaiModel(model),
    selectCustom: () => selectOpenaiCustomModel(),
    setCustom: (model) => setOpenaiCustomModel(model),
  },
  groq: {
    model: (settings) => settings.groq_model,
    modelKind: (settings) => settings.groq_model_kind,
    customModel: (settings) => settings.groq_custom_model,
    selectStandard: (model) => selectGroqModel(model),
    selectCustom: () => selectGroqCustomModel(),
    setCustom: (model) => setGroqCustomModel(model),
  },
};

function selectionFromSettings(
  kind: AppSettings["openai_model_kind"],
  model: string
): CloudSelection | null {
  if (kind === "custom") return { kind: "custom" };
  if (kind === "standard") return { kind: "standard", id: model };
  return null;
}

function providerSettingsPatch(
  provider: CloudProviderId,
  settings: AppSettings
): Partial<AppSettings> {
  return provider === "openai"
    ? {
        openai_model: settings.openai_model,
        openai_model_kind: settings.openai_model_kind,
        openai_custom_model: settings.openai_custom_model,
      }
    : {
        groq_model: settings.groq_model,
        groq_model_kind: settings.groq_model_kind,
        groq_custom_model: settings.groq_custom_model,
      };
}

export function useCloudProvider({
  provider,
  settings,
  applySettings,
  setError,
}: {
  provider: CloudProviderId;
  settings: AppSettings;
  applySettings: ApplySettings;
  setError: SetError;
}) {
  const config = CONFIG[provider];
  const keyDraft = useDirtyText("");
  const customDraft = useDirtyText(config.customModel(settings));
  const [showKey, setShowKey] = useState(false);
  const [selection, setSelection] = useState<CloudSelection | null>(() =>
    selectionFromSettings(config.modelKind(settings), config.model(settings))
  );
  const [persistedSelection, setPersistedSelection] = useState<CloudSelection | null>(() =>
    selectionFromSettings(config.modelKind(settings), config.model(settings))
  );
  const modelQueue = useRef<Promise<void>>(Promise.resolve());
  const keyQueue = useRef<Promise<void>>(Promise.resolve());
  const persistedSettingsRef = useRef(settings);
  const selectionRequestRef = useRef(0);
  const keyLoadRequestRef = useRef(0);

  const commitProviderSettings = useCallback(
    (next: AppSettings) => {
      persistedSettingsRef.current = next;
      setPersistedSelection(
        selectionFromSettings(config.modelKind(next), config.model(next))
      );
      applySettings(providerSettingsPatch(provider, next));
    },
    [applySettings, config, provider]
  );

  const restorePersistedSelection = useCallback(() => {
    const persisted = persistedSettingsRef.current;
    setSelection(
      selectionFromSettings(config.modelKind(persisted), config.model(persisted))
    );
  }, [config]);

  useEffect(() => {
    const request = ++keyLoadRequestRef.current;
    getApiKey(provider)
      .then((key) => {
        if (keyLoadRequestRef.current === request) {
          keyDraft.loadPersisted(key);
        }
      })
      .catch(() => {
        if (keyLoadRequestRef.current === request) {
          keyDraft.loadPersisted("");
        }
      });
    return () => {
      if (keyLoadRequestRef.current === request) {
        keyLoadRequestRef.current += 1;
      }
    };
  }, [keyDraft.loadPersisted, provider]);

  const flushKey = useCallback(
    () =>
      enqueue(keyQueue, async () => {
        while (keyDraft.dirtyRef.current) {
          const draft = keyDraft.valueRef.current;
          const key = draft.trim();
          if (key === "") {
            await deleteApiKey(provider);
          } else {
            await setApiKey(provider, key);
          }
          keyDraft.markPersisted(draft);
        }
      }),
    [keyDraft.dirtyRef, keyDraft.markPersisted, keyDraft.valueRef, provider]
  );

  const flushCustomModel = useCallback(
    () =>
      enqueue(modelQueue, async () => {
        while (customDraft.dirtyRef.current) {
          const draft = customDraft.valueRef.current;
          const next = await config.setCustom(draft);
          commitProviderSettings(next);
          customDraft.markPersisted(draft);
        }
      }),
    [
      commitProviderSettings,
      config,
      customDraft.dirtyRef,
      customDraft.markPersisted,
      customDraft.valueRef,
    ]
  );

  const saveKey = useCallback(() => {
    void flushKey().catch((cause) => setError(errorMessageForCode(cause)));
  }, [flushKey, setError]);

  const saveCustomModel = useCallback(() => {
    void flushCustomModel().catch((cause) =>
      setError(errorMessageForCode(cause))
    );
  }, [flushCustomModel, setError]);

  const selectStandardModel = useCallback(
    (modelId: string) => {
      const request = ++selectionRequestRef.current;
      setSelection({ kind: "standard", id: modelId });
      const operation = enqueue(modelQueue, async () => {
        const next = await config.selectStandard(modelId);
        commitProviderSettings(next);
      });
      void operation.catch((cause) => {
        if (selectionRequestRef.current === request) {
          restorePersistedSelection();
        }
        setError(errorMessageForCode(cause));
      });
    },
    [commitProviderSettings, config, restorePersistedSelection, setError]
  );

  const selectCustomModel = useCallback(() => {
    const request = ++selectionRequestRef.current;
    setSelection({ kind: "custom" });
    const operation = enqueue(modelQueue, async () => {
      const next = await config.selectCustom();
      commitProviderSettings(next);
    });
    void operation.catch((cause) => {
      if (selectionRequestRef.current === request) {
        restorePersistedSelection();
      }
      setError(errorMessageForCode(cause));
    });
  }, [commitProviderSettings, config, restorePersistedSelection, setError]);

  const isConfigured = (() => {
    const selectedModel =
      persistedSelection?.kind === "custom"
        ? customDraft.value
        : persistedSelection?.kind === "standard"
          ? persistedSelection.id ?? ""
          : "";
    return (
      persistedSelection !== null &&
      keyDraft.value.trim() !== "" &&
      selectedModel.length > 0
    );
  })();


  const changeKey = useCallback(
    (next: string) => {
      keyLoadRequestRef.current += 1;
      keyDraft.setDraft(next);
    },
    [keyDraft.setDraft]
  );
  const flushDirtyFields = useCallback(
    async () => {
      await Promise.all([flushKey(), flushCustomModel()]);
    },
    [flushCustomModel, flushKey]
  );

  return {
    selection,
    customModelInput: customDraft.value,
    apiKey: keyDraft.value,
    showKey,
    isConfigured,
    selectStandardModel,
    selectCustomModel,
    changeCustomModel: customDraft.setDraft,
    saveCustomModel,
    changeKey,
    saveKey,
    toggleShowKey: () => setShowKey((current) => !current),
    flushDirtyFields,
  };
}
