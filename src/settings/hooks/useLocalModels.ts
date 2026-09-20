import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  cancelDownload,
  deleteModel,
  downloadModel,
  listLocalModels,
  selectLocalCustomModel,
  selectLocalModel,
  setLocalCustomModelPath,
} from "../../lib/api";
import { LOCAL_MODEL_SLOTS, type LocalModel } from "../../lib/constants";
import { errorMessageForCode } from "../../lib/errors";
import {
  downloadErrorFields,
  isDownloadCancelled,
  isDownloadComplete,
  isDownloadProgress,
  type AppSettings,
  type DownloadUiState,
  type LocalModelInfo,
} from "../../lib/types";
import { useDirtyText } from "../../hooks/useDirtyText";
import { useTauriEvent } from "../../hooks/useTauriEvent";
import { enqueue } from "./queue";

type LocalSelection =
  | { kind: "standard"; downloadPath: string }
  | { kind: "custom" }
  | null;
type DeleteCandidate = { downloadPath: string } | null;
type ApplySettings = (patch: Partial<AppSettings>) => void;
type SetError = (message: string | null) => void;

const DOWNLOAD_SUCCESS_HOLD_MS = 1_000;
const LOCAL_DOWNLOAD_PATHS = LOCAL_MODEL_SLOTS.flatMap((model) =>
  model ? [model.downloadPath] : []
);

function localSettingsPatch(settings: AppSettings): Partial<AppSettings> {
  return {
    local_model_path: settings.local_model_path,
    local_model_kind: settings.local_model_kind,
    local_custom_model_path: settings.local_custom_model_path,
  };
}

function localSelectionFromSettings(
  settings: Pick<AppSettings, "local_model_path" | "local_model_kind">,
  infos: LocalModelInfo[]
): LocalSelection {
  if (settings.local_model_kind === "custom") {
    return { kind: "custom" };
  }
  if (settings.local_model_kind !== "standard") {
    return null;
  }
  if (!settings.local_model_path) {
    const first = LOCAL_MODEL_SLOTS[0];
    return first ? { kind: "standard", downloadPath: first.downloadPath } : null;
  }
  const info = infos.find((model) => model.path === settings.local_model_path);
  return {
    kind: "standard",
    downloadPath: info?.download_path ?? settings.local_model_path,
  };
}

export function useLocalModels({
  settings,
  applySettings,
  setError,
}: {
  settings: AppSettings;
  applySettings: ApplySettings;
  setError: SetError;
}) {
  const [localInfos, setLocalInfos] = useState<LocalModelInfo[]>([]);
  const [downloadState, setDownloadStateValue] = useState<DownloadUiState>({
    phase: "idle",
  });
  const downloadStateRef = useRef<DownloadUiState>(downloadState);
  const [deleteCandidate, setDeleteCandidate] =
    useState<DeleteCandidate>(null);
  const [deletePending, setDeletePending] = useState(false);
  const deleteTriggerRef = useRef<HTMLButtonElement | null>(null);
  const cancelDeleteRef = useRef<HTMLButtonElement | null>(null);
  const confirmDeleteRef = useRef<HTMLButtonElement | null>(null);
  const customPathDraft = useDirtyText(
    settings.local_custom_model_path,
    settings.local_custom_model_path
  );
  const [localSelection, setLocalSelection] = useState<LocalSelection>(() =>
    localSelectionFromSettings(settings, [])
  );
  const [persistedLocalSelection, setPersistedLocalSelection] = useState<LocalSelection>(() =>
    localSelectionFromSettings(settings, [])
  );
  const localQueue = useRef<Promise<void>>(Promise.resolve());
  const persistedSettingsRef = useRef(settings);
  const selectionRequestRef = useRef(0);
  const localListRequestRef = useRef(0);

  const commitLocalSettings = useCallback(
    (next: AppSettings) => {
      persistedSettingsRef.current = next;
      setPersistedLocalSelection(localSelectionFromSettings(next, localInfos));
      applySettings(localSettingsPatch(next));
    },
    [applySettings, localInfos]
  );

  const transitionDownload = useCallback(
    (
      update:
        | DownloadUiState
        | ((current: DownloadUiState) => DownloadUiState)
    ) => {
      const next =
        typeof update === "function"
          ? update(downloadStateRef.current)
          : update;
      downloadStateRef.current = next;
      setDownloadStateValue(next);
    },
    []
  );

  const refreshLocal = useCallback(() => {
    const request = ++localListRequestRef.current;
    listLocalModels(LOCAL_DOWNLOAD_PATHS)
      .then((infos) => {
        if (localListRequestRef.current === request) {
          setLocalInfos(infos);
        }
      })
      .catch(() => {
        if (localListRequestRef.current === request) {
          setError(errorMessageForCode("model_download"));
        }
      });
  }, [setError]);

  useEffect(() => {
    refreshLocal();
    return () => {
      localListRequestRef.current += 1;
    };
  }, [refreshLocal]);

  useEffect(() => {
    persistedSettingsRef.current = {
      ...persistedSettingsRef.current,
      ...localSettingsPatch(settings),
    };
    const persisted = localSelectionFromSettings(settings, localInfos);
    setPersistedLocalSelection(persisted);
    setLocalSelection(persisted);
  }, [
    localInfos,
    settings.local_custom_model_path,
    settings.local_model_kind,
    settings.local_model_path,
  ]);

  useTauriEvent<unknown>("model:download:progress", (payload) => {
    if (!isDownloadProgress(payload)) return;
    const current = downloadStateRef.current;
    if (
      current.phase !== "downloading" ||
      current.downloadPath !== payload.download_path
    ) {
      return;
    }
    transitionDownload({
      ...current,
      downloadedBytes: payload.downloaded_bytes,
      totalBytes: payload.total_bytes,
      progressReceived: true,
    });
  });

  useTauriEvent<unknown>("model:download:complete", (payload) => {
    if (!isDownloadComplete(payload)) return;
    const current = downloadStateRef.current;
    if (
      current.phase !== "downloading" ||
      current.downloadPath !== payload.download_path
    ) {
      return;
    }
    setLocalInfos((infos) =>
      infos.map((info) =>
        info.download_path === payload.download_path
          ? {
              ...info,
              path: payload.path,
              downloaded: true,
              size_bytes: current.downloadedBytes,
            }
          : info
      )
    );
    transitionDownload({
      phase: "success",
      downloadPath: payload.download_path,
    });
    refreshLocal();
  });

  useTauriEvent<unknown>("model:download:cancelled", (payload) => {
    if (!isDownloadCancelled(payload)) return;
    const current = downloadStateRef.current;
    if (
      current.phase !== "downloading" ||
      current.downloadPath !== payload.download_path
    ) {
      return;
    }
    transitionDownload({ phase: "idle" });
    refreshLocal();
  });

  useTauriEvent<unknown>("model:download:error", (payload) => {
    const fields = downloadErrorFields(payload);
    if (!fields) return;
    const current = downloadStateRef.current;
    if (
      current.phase !== "downloading" ||
      current.downloadPath !== fields.download_path
    ) {
      return;
    }
    transitionDownload({ phase: "idle" });
    setError(errorMessageForCode(fields.code));
    refreshLocal();
  });

  useEffect(() => {
    if (downloadState.phase !== "success") return;
    const completedPath = downloadState.downloadPath;
    const timeout = window.setTimeout(() => {
      const current = downloadStateRef.current;
      if (
        current.phase === "success" &&
        current.downloadPath === completedPath
      ) {
        transitionDownload({ phase: "idle" });
      }
    }, DOWNLOAD_SUCCESS_HOLD_MS);
    return () => window.clearTimeout(timeout);
  }, [downloadState, transitionDownload]);

  useEffect(() => {
    if (deleteCandidate) cancelDeleteRef.current?.focus();
  }, [deleteCandidate]);

  const flushCustomPath = useCallback(
    () =>
      enqueue(localQueue, async () => {
        while (customPathDraft.dirtyRef.current) {
          const draft = customPathDraft.valueRef.current;
          const next = await setLocalCustomModelPath(draft);
          commitLocalSettings(next);
          customPathDraft.markPersisted(draft);
        }
      }),
    [
      commitLocalSettings,
      customPathDraft.dirtyRef,
      customPathDraft.markPersisted,
      customPathDraft.valueRef,
    ]
  );

  const saveCustomPath = useCallback(() => {
    void flushCustomPath().catch((cause) =>
      setError(errorMessageForCode(cause))
    );
  }, [flushCustomPath, setError]);

  const selectStandardModel = useCallback(
    async (model: LocalModel) => {
      let info = localInfos.find(
        (candidate) => candidate.download_path === model.downloadPath
      );
      let listRequest: number | null = null;
      try {
        if (!info) {
          listRequest = ++localListRequestRef.current;
          const refreshed = await listLocalModels(LOCAL_DOWNLOAD_PATHS);
          if (localListRequestRef.current !== listRequest) {
            return;
          }
          setLocalInfos(refreshed);
          info = refreshed.find(
            (candidate) => candidate.download_path === model.downloadPath
          );
        }
        if (!info) return;
        const selectedInfo = info;

        const request = ++selectionRequestRef.current;
        setLocalSelection({ kind: "standard", downloadPath: model.downloadPath });
        try {
          await enqueue(localQueue, async () => {
            const next = await selectLocalModel(selectedInfo.path);
            commitLocalSettings(next);
          });
        } catch (cause) {
          if (selectionRequestRef.current === request) {
            setLocalSelection(
              localSelectionFromSettings(persistedSettingsRef.current, localInfos)
            );
          }
          throw cause;
        }
      } catch (cause) {
        if (listRequest !== null && localListRequestRef.current !== listRequest) {
          return;
        }
        setError(errorMessageForCode(cause));
      }
    },
    [commitLocalSettings, localInfos, setError]
  );

  const selectCustomModel = useCallback(async () => {
    const request = ++selectionRequestRef.current;
    setLocalSelection({ kind: "custom" });
    try {
      await enqueue(localQueue, async () => {
        const next = await selectLocalCustomModel();
        commitLocalSettings(next);
      });
    } catch (cause) {
      if (selectionRequestRef.current === request) {
        setLocalSelection(
          localSelectionFromSettings(persistedSettingsRef.current, localInfos)
        );
      }
      setError(errorMessageForCode(cause));
    }
  }, [commitLocalSettings, localInfos, setError]);

  const pickCustomBin = useCallback(async () => {
    try {
      const picked = await openDialog({
        multiple: false,
        directory: false,
        filters: [{ name: "Whisper Model (.bin)", extensions: ["bin"] }],
      });
      if (typeof picked === "string") {
        customPathDraft.setDraft(picked);
        await enqueue(localQueue, async () => {
          const next = await setLocalCustomModelPath(picked);
          commitLocalSettings(next);
          customPathDraft.markPersisted(picked);
        });
      }
    } catch (cause) {
      setError(errorMessageForCode(cause));
    }
  }, [
    commitLocalSettings,
    customPathDraft.markPersisted,
    customPathDraft.setDraft,
    setError,
  ]);

  const startDownload = useCallback(
    async (downloadPath: string) => {
      if (downloadStateRef.current.phase !== "idle") return;
      setError(null);
      transitionDownload({
        phase: "downloading",
        downloadPath,
        downloadedBytes: 0,
        totalBytes: null,
        progressReceived: false,
        cancelRequested: false,
      });
      try {
        await downloadModel(downloadPath);
      } catch (cause) {
        const current = downloadStateRef.current as DownloadUiState;
        if (
          current.phase === "downloading" &&
          current.downloadPath === downloadPath
        ) {
          transitionDownload({ phase: "idle" });
        }
        setError(errorMessageForCode(cause));
      }
    },
    [setError, transitionDownload]
  );

  const cancelActiveDownload = useCallback(
    async (downloadPath: string) => {
      const current = downloadStateRef.current;
      if (
        current.phase !== "downloading" ||
        current.downloadPath !== downloadPath ||
        current.cancelRequested
      ) {
        return;
      }
      transitionDownload({ ...current, cancelRequested: true });
      try {
        await cancelDownload(downloadPath);
      } catch (cause) {
        const latest = downloadStateRef.current;
        if (
          latest.phase === "downloading" &&
          latest.downloadPath === downloadPath
        ) {
          transitionDownload({ ...latest, cancelRequested: false });
        }
        setError(errorMessageForCode(cause));
      }
    },
    [setError, transitionDownload]
  );

  const closeDeleteModal = useCallback(() => {
    const trigger = deleteTriggerRef.current;
    setDeleteCandidate(null);
    queueMicrotask(() => trigger?.focus());
  }, []);

  const openDeleteModal = useCallback(
    (downloadPath: string, trigger: HTMLButtonElement) => {
      deleteTriggerRef.current = trigger;
      setDeleteCandidate({ downloadPath });
      setDeletePending(false);
    },
    []
  );

  const confirmDelete = useCallback(async () => {
    if (!deleteCandidate || deletePending) return;
    const { downloadPath } = deleteCandidate;
    setDeletePending(true);
    setError(null);
    try {
      await enqueue(localQueue, async () => {
        const next = await deleteModel(downloadPath);
        commitLocalSettings(next);
        setLocalSelection(localSelectionFromSettings(next, localInfos));
      });
      setLocalInfos((infos) =>
        infos.map((info) =>
          info.download_path === downloadPath
            ? { ...info, downloaded: false, size_bytes: null }
            : info
        )
      );
      const current = downloadStateRef.current;
      if (
        current.phase === "success" &&
        current.downloadPath === downloadPath
      ) {
        transitionDownload({ phase: "idle" });
      }
      closeDeleteModal();
      refreshLocal();
    } catch (cause) {
      setError(errorMessageForCode(cause));
      refreshLocal();
    } finally {
      setDeletePending(false);
    }
  }, [
    closeDeleteModal,
    commitLocalSettings,
    deleteCandidate,
    deletePending,
    localInfos,
    refreshLocal,
    setError,
    transitionDownload,
  ]);

  const handleDeleteModalKeyDown = useCallback(
    (event: KeyboardEvent<HTMLDivElement>) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeDeleteModal();
        return;
      }
      if (event.key !== "Tab") return;
      const controls = [cancelDeleteRef.current, confirmDeleteRef.current].filter(
        (control): control is HTMLButtonElement =>
          control !== null && !control.disabled
      );
      if (controls.length === 0) return;
      const currentIndex = controls.indexOf(
        document.activeElement as HTMLButtonElement
      );
      const nextIndex = event.shiftKey
        ? currentIndex <= 0
          ? controls.length - 1
          : currentIndex - 1
        : currentIndex === -1 || currentIndex === controls.length - 1
          ? 0
          : currentIndex + 1;
      event.preventDefault();
      controls[nextIndex].focus();
    },
    [closeDeleteModal]
  );

  const isLocalModelSelected = useCallback(
    (model: LocalModel) =>
      localSelection?.kind === "standard" &&
      localSelection.downloadPath === model.downloadPath,
    [localSelection]
  );

  const isConfigured = (() => {
    if (persistedLocalSelection?.kind === "custom") {
      return customPathDraft.value.length > 0;
    }
    if (persistedLocalSelection?.kind === "standard") {
      return (
        localInfos.find(
          (info) => info.download_path === persistedLocalSelection.downloadPath
        )?.downloaded === true
      );
    }
    return false;
  })();

  return {
    localInfos,
    downloadState,
    customPathInput: customPathDraft.value,
    isCustomLocal: localSelection?.kind === "custom",
    isConfigured,
    isLocalModelSelected,
    selectStandardModel,
    selectCustomModel,
    changeCustomPath: customPathDraft.setDraft,
    saveCustomPath,
    pickCustomBin,
    startDownload,
    cancelActiveDownload,
    openDeleteModal,
    deleteCandidate,
    deletePending,
    cancelDeleteRef,
    confirmDeleteRef,
    confirmDelete,
    closeDeleteModal,
    handleDeleteModalKeyDown,
    flushDirtyFields: flushCustomPath,
  };
}
