//! Managed Local-model storage identity, accounting, and deletion primitives.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::DownloadError;

pub(super) const MAX_DOWNLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub(super) const MAX_MODELS_DIR_BYTES: u64 = 8 * 1024 * 1024 * 1024;

pub(super) fn ensure_models_dir(models_dir: &Path) -> Result<(), DownloadError> {
    std::fs::create_dir_all(models_dir).map_err(|_| DownloadError::Storage)
}

/// Safe deterministic filename for one exact download path.
pub(super) fn storage_file_name(download_path: &str) -> String {
    let digest = Sha256::digest(download_path.as_bytes());
    format!("{digest:x}.bin")
}

pub(super) fn model_path(models_dir: &Path, download_path: &str) -> PathBuf {
    models_dir.join(storage_file_name(download_path))
}

pub(super) fn partial_path(models_dir: &Path, download_path: &str) -> PathBuf {
    models_dir.join(format!("{}.partial", storage_file_name(download_path)))
}

const STATE_DIR_NAME: &str = ".state";
const PENDING_SELECTED_DELETION_FILE: &str = "pending-selected-standard-deletion";

fn pending_selected_deletion_path(models_dir: &Path) -> PathBuf {
    models_dir
        .join(STATE_DIR_NAME)
        .join(PENDING_SELECTED_DELETION_FILE)
}

pub(super) fn write_pending_selected_deletion(
    models_dir: &Path,
    model_path: &Path,
) -> Result<(), DownloadError> {
    let marker = pending_selected_deletion_path(models_dir);
    let parent = marker.parent().ok_or(DownloadError::Storage)?;
    std::fs::create_dir_all(parent).map_err(|_| DownloadError::Storage)?;
    std::fs::write(marker, model_path.to_string_lossy().as_bytes())
        .map_err(|_| DownloadError::Storage)
}

pub(super) fn read_pending_selected_deletion(
    models_dir: &Path,
) -> Result<Option<PathBuf>, DownloadError> {
    let marker = pending_selected_deletion_path(models_dir);
    match std::fs::read_to_string(marker) {
        Ok(path) => Ok(Some(PathBuf::from(path))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(DownloadError::Storage),
    }
}

pub(super) fn clear_pending_selected_deletion(models_dir: &Path) -> Result<(), DownloadError> {
    remove_file_if_exists(&pending_selected_deletion_path(models_dir))
        .map_err(|_| DownloadError::Storage)
}

pub(super) fn directory_usage_bytes(models_dir: &Path) -> Result<u64, DownloadError> {
    let entries = std::fs::read_dir(models_dir).map_err(|_| DownloadError::Storage)?;
    let mut used = 0_u64;
    for entry in entries {
        let entry = entry.map_err(|_| DownloadError::Storage)?;
        let metadata = entry.metadata().map_err(|_| DownloadError::Storage)?;
        if metadata.is_file() {
            used = used
                .checked_add(metadata.len())
                .ok_or(DownloadError::Storage)?;
        }
    }
    Ok(used)
}

pub(super) fn download_budget(used: u64) -> Result<u64, DownloadError> {
    let remaining = MAX_MODELS_DIR_BYTES
        .checked_sub(used)
        .ok_or(DownloadError::Storage)?;
    if remaining == 0 {
        return Err(DownloadError::Storage);
    }
    Ok(remaining.min(MAX_DOWNLOAD_BYTES))
}

pub(super) fn available_download_bytes(models_dir: &Path) -> Result<u64, DownloadError> {
    download_budget(directory_usage_bytes(models_dir)?)
}

pub(super) fn file_metadata(path: &Path) -> Option<std::fs::Metadata> {
    std::fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
}

pub(super) fn remove_file_if_exists(path: &Path) -> Result<(), std::io::Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn remove_final_file(path: &Path) -> Result<(), DownloadError> {
    std::fs::remove_file(path).map_err(|_| DownloadError::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL_PATH: &str =
        "https://models.example.test/whisper/environment-alpha.bin?source=environment";
    const MEDIUM_PATH: &str = "https://another.example.test/models/environment-beta.bin";

    #[test]
    fn storage_identity_is_stable_and_path_specific() {
        let small = storage_file_name(SMALL_PATH);
        assert_eq!(small, storage_file_name(SMALL_PATH));
        assert_ne!(small, storage_file_name(MEDIUM_PATH));
        assert!(small.ends_with(".bin"));
    }

    #[test]
    fn source_text_cannot_escape_models_directory() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let path = model_path(&models_dir, "../../outside.bin");
        assert_eq!(path.parent(), Some(models_dir.as_path()));
        assert!(!path.file_name().unwrap().to_string_lossy().contains(".."));
    }

    #[test]
    fn download_budget_enforces_per_file_and_directory_limits() {
        assert_eq!(download_budget(0).unwrap(), MAX_DOWNLOAD_BYTES);
        assert_eq!(download_budget(MAX_MODELS_DIR_BYTES - 7).unwrap(), 7);
        assert!(download_budget(MAX_MODELS_DIR_BYTES).is_err());
        assert!(download_budget(MAX_MODELS_DIR_BYTES + 1).is_err());
    }

    #[test]
    fn directory_usage_counts_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        ensure_models_dir(&models_dir).unwrap();
        std::fs::write(models_dir.join("one.bin"), b"12345").unwrap();
        std::fs::write(models_dir.join("two.partial"), b"123").unwrap();
        std::fs::create_dir(models_dir.join("nested")).unwrap();
        assert_eq!(directory_usage_bytes(&models_dir).unwrap(), 8);
    }
}
