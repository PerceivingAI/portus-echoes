//! Environment-defined Local model management/downloads (`docs/DOWNLOADS.md`).
//!
//! Rust owns the build-time URL allowlist. The frontend can operate only on an
//! exact configured HTTPS path; the path is hashed into a safe filename under
//! the app config `models/` directory. Progress streams to the frontend via
//! `model:download:progress|complete|cancelled|error` events. Only one bounded
//! download runs at a time. Files land in a `.partial` sibling first and are
//! renamed on completion, so a cancelled or failed download never leaves a
//! usable partial model.

use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;

mod storage;
mod transfer;

use crate::models::UserErrorCode;
#[cfg(test)]
use storage::{MAX_DOWNLOAD_BYTES, MAX_MODELS_DIR_BYTES};
use transfer::{download_to_disk, DownloadFailure};

/// Exact Local model URLs compiled from the same Vite environment used by the
/// frontend build. `build.rs` validates and serializes this value.
pub fn configured_download_paths() -> Vec<String> {
    serde_json::from_str(env!("PORTUS_LOCAL_MODEL_URLS"))
        .expect("build.rs must emit a valid Local model URL list")
}

/// Deterministic managed path for build-time Local Standard slot 1.
/// The file need not exist; selection and download/readiness are independent.
pub(crate) fn first_configured_model_path(models_dir: &Path) -> Option<PathBuf> {
    configured_download_paths()
        .first()
        .map(|download_path| storage::model_path(models_dir, download_path))
}

/// Journal a managed deletion before the file is removed. The journal exists
/// only to recover the narrow case where deletion commits but the later
/// selected-Settings cleanup cannot be persisted.
pub(crate) fn begin_selected_deletion_recovery(
    models_dir: &Path,
    model_path: &Path,
) -> Result<(), DownloadError> {
    storage::write_pending_selected_deletion(models_dir, model_path)
}

pub(crate) fn pending_selected_deletion_recovery(
    models_dir: &Path,
) -> Result<Option<PathBuf>, DownloadError> {
    storage::read_pending_selected_deletion(models_dir)
}

pub(crate) fn clear_selected_deletion_recovery(models_dir: &Path) -> Result<(), DownloadError> {
    storage::clear_pending_selected_deletion(models_dir)
}

/// Disk state of one environment-defined Local model.
#[derive(Debug, Clone, Serialize)]
pub struct LocalModelInfo {
    pub download_path: String,
    pub path: String,
    pub downloaded: bool,
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Serialize)]
struct ProgressPayload {
    download_path: String,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
}

#[derive(Clone, Serialize)]
struct CompletePayload {
    download_path: String,
    path: String,
}

#[derive(Clone, Serialize)]
struct CancelledPayload {
    download_path: String,
}

#[derive(Clone, Serialize)]
struct ErrorPayload {
    download_path: String,
    code: UserErrorCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadError {
    InvalidConfiguration,
    Unavailable,
    Storage,
    AlreadyDownloaded,
    Busy,
    NoActiveDownload,
    DeleteWhileActive,
    NotDownloaded,
}

struct ActiveDownload {
    download_path: String,
    cancel: CancellationToken,
}

/// Owns the single-download slot, configured URL allowlist, and models directory.
pub struct DownloadManager {
    models_dir: PathBuf,
    configured_download_paths: Box<[String]>,
    active: Mutex<Option<ActiveDownload>>,
}

impl DownloadManager {
    pub fn new(
        models_dir: PathBuf,
        configured_download_paths: Vec<String>,
    ) -> Result<Self, DownloadError> {
        if configured_download_paths.len() > 2 {
            return Err(DownloadError::InvalidConfiguration);
        }
        for (index, download_path) in configured_download_paths.iter().enumerate() {
            let url = reqwest::Url::parse(download_path)
                .map_err(|_| DownloadError::InvalidConfiguration)?;
            if url.scheme() != "https" || url.host_str().is_none() {
                return Err(DownloadError::InvalidConfiguration);
            }
            if configured_download_paths[..index].contains(download_path) {
                return Err(DownloadError::InvalidConfiguration);
            }
        }
        Ok(Self {
            models_dir,
            configured_download_paths: configured_download_paths.into_boxed_slice(),
            active: Mutex::new(None),
        })
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    /// Safe deterministic filename for one exact download path.
    #[cfg(test)]
    fn storage_file_name(download_path: &str) -> String {
        storage::storage_file_name(download_path)
    }

    /// Local destination for a download path, whether downloaded or not.
    pub fn model_path(&self, download_path: &str) -> PathBuf {
        storage::model_path(&self.models_dir, download_path)
    }

    /// Whether a renderer-supplied Standard Local selection is one of the
    /// deterministic destinations owned by the build-time managed model list.
    pub fn is_managed_model_path(&self, path: &Path) -> bool {
        self.configured_download_paths
            .iter()
            .any(|download_path| self.model_path(download_path) == path)
    }

    fn partial_path(&self, download_path: &str) -> PathBuf {
        storage::partial_path(&self.models_dir, download_path)
    }

    pub(crate) fn ensure_configured(&self, download_path: &str) -> Result<(), DownloadError> {
        if self
            .configured_download_paths
            .iter()
            .any(|configured| configured == download_path)
        {
            Ok(())
        } else {
            Err(DownloadError::Unavailable)
        }
    }

    #[cfg(test)]
    fn download_budget(used: u64) -> Result<u64, DownloadError> {
        storage::download_budget(used)
    }

    fn available_download_bytes(&self) -> Result<u64, DownloadError> {
        storage::available_download_bytes(&self.models_dir)
    }

    /// Disk state for the build-time configured model list.
    pub fn list(&self) -> Vec<LocalModelInfo> {
        self.configured_download_paths
            .iter()
            .map(|download_path| {
                let path = self.model_path(download_path);
                let meta = storage::file_metadata(&path);
                LocalModelInfo {
                    download_path: download_path.clone(),
                    path: path.to_string_lossy().to_string(),
                    downloaded: meta.is_some(),
                    size_bytes: meta.map(|metadata| metadata.len()),
                }
            })
            .collect()
    }

    /// Whether any download is currently running.
    #[allow(dead_code)] // used by tests; exposed for future UI state
    pub fn is_downloading(&self) -> bool {
        self.active.lock().is_some()
    }

    /// Prepare one exact managed download after winning the single-download slot.
    /// A stale partial for this same requested model is incomplete app-owned
    /// state from an earlier interrupted attempt, so remove it before measuring
    /// the replacement attempt's storage budget. Acquiring the slot first keeps
    /// an active download's partial file protected from another start attempt.
    fn prepare_start(
        &self,
        download_path: &str,
    ) -> Result<(PathBuf, PathBuf, CancellationToken, u64), DownloadError> {
        self.ensure_configured(download_path)?;
        let final_path = self.model_path(download_path);
        storage::ensure_models_dir(&self.models_dir)?;
        if final_path.is_file() {
            return Err(DownloadError::AlreadyDownloaded);
        }

        let cancel = self.acquire_slot(download_path)?;
        let partial_path = self.partial_path(download_path);
        let prepared = (|| {
            storage::remove_file_if_exists(&partial_path).map_err(|_| DownloadError::Storage)?;
            let max_bytes = self.available_download_bytes()?;
            Ok((final_path, partial_path, cancel, max_bytes))
        })();
        if prepared.is_err() {
            self.finish(download_path);
        }
        prepared
    }

    /// Start downloading one exact configured path in a cancellation-aware task.
    /// Returns immediately; progress arrives via events. Errors when another
    /// download is already running or the storage budget would be exceeded.
    pub fn start(
        self: &Arc<Self>,
        app: &AppHandle,
        download_path: &str,
    ) -> Result<(), DownloadError> {
        let (final_path, partial_path, cancel, max_bytes) = self.prepare_start(download_path)?;
        let manager = ManagerHandle {
            app: app.clone(),
            download_path: download_path.to_string(),
        };
        let this = Arc::clone(self);
        let source = download_path.to_string();
        tauri::async_runtime::spawn(async move {
            let outcome = download_to_disk(
                &source,
                &partial_path,
                &final_path,
                &cancel,
                max_bytes,
                |downloaded_bytes, total_bytes| {
                    manager.emit(
                        "model:download:progress",
                        ProgressPayload {
                            download_path: manager.download_path.clone(),
                            downloaded_bytes,
                            total_bytes,
                        },
                    );
                },
            )
            .await;

            this.finish(&source);
            match outcome {
                Ok(()) => {
                    manager.emit(
                        "model:download:complete",
                        CompletePayload {
                            download_path: manager.download_path.clone(),
                            path: final_path.to_string_lossy().to_string(),
                        },
                    );
                    crate::app::local_models::preload_completed_download_if_selected(
                        &manager.app,
                        &final_path,
                    );
                }
                Err(DownloadFailure::Cancelled) => manager.emit(
                    "model:download:cancelled",
                    CancelledPayload {
                        download_path: manager.download_path.clone(),
                    },
                ),
                Err(DownloadFailure::Failed) => manager.emit(
                    "model:download:error",
                    ErrorPayload {
                        download_path: manager.download_path.clone(),
                        code: UserErrorCode::ModelDownload,
                    },
                ),
            }
        });
        Ok(())
    }

    /// Occupy the single download slot. Returns its cancellation token.
    fn acquire_slot(&self, download_path: &str) -> Result<CancellationToken, DownloadError> {
        let cancel = CancellationToken::new();
        let mut guard = self.active.lock();
        if guard.is_some() {
            return Err(DownloadError::Busy);
        }
        *guard = Some(ActiveDownload {
            download_path: download_path.to_string(),
            cancel: cancel.clone(),
        });
        Ok(cancel)
    }

    /// Cancel the active download. No-op when the exact path does not match.
    pub fn cancel(&self, download_path: &str) -> Result<(), DownloadError> {
        self.ensure_configured(download_path)?;
        let guard = self.active.lock();
        match guard.as_ref() {
            Some(active) if active.download_path == download_path => {
                active.cancel.cancel();
                Ok(())
            }
            _ => Err(DownloadError::NoActiveDownload),
        }
    }

    /// Clear the active slot when a download thread finishes.
    fn finish(&self, download_path: &str) {
        let mut guard = self.active.lock();
        if guard
            .as_ref()
            .is_some_and(|active| active.download_path == download_path)
        {
            *guard = None;
        }
    }

    /// Delete a downloaded model and any stale partial file for its exact path.
    pub fn delete(&self, download_path: &str) -> Result<PathBuf, DownloadError> {
        self.ensure_configured(download_path)?;
        let active_guard = self.active.lock();
        if active_guard
            .as_ref()
            .is_some_and(|active| active.download_path == download_path)
        {
            return Err(DownloadError::DeleteWhileActive);
        }

        let path = self.model_path(download_path);
        let partial_path = self.partial_path(download_path);
        let downloaded = path.is_file();
        storage::remove_file_if_exists(&partial_path).map_err(|_| DownloadError::Storage)?;
        if !downloaded {
            return Err(DownloadError::NotDownloaded);
        }
        storage::remove_final_file(&path)?;
        drop(active_guard);
        Ok(path)
    }
}

/// Thin event emitter carried into the download thread.
struct ManagerHandle {
    app: AppHandle,
    download_path: String,
}

impl ManagerHandle {
    fn emit(&self, event: &str, payload: impl Serialize + Clone) {
        let _ = self.app.emit(event, payload);
    }
}

#[cfg(test)]
mod tests;
