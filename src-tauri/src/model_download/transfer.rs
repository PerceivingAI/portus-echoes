//! Cancellation-aware HTTP transfer into one managed Local-model artifact.

use std::path::Path;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum DownloadFailure {
    Cancelled,
    Failed,
}

/// Authoritative timeout for one managed model-download HTTP request/response stream.
pub(super) const MANAGED_DOWNLOAD_REQUEST_TIMEOUT: Duration = Duration::from_secs(30 * 60);

async fn remove_partial_file(partial_path: &Path) -> Result<(), ()> {
    match tokio::fs::remove_file(partial_path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(()),
    }
}

/// Stream the download to `<file>.partial`, then atomically rename it into
/// place. Every failure, including cancellation, removes the partial file.
pub(super) async fn download_to_disk(
    download_path: &str,
    partial_path: &Path,
    final_path: &Path,
    cancel: &CancellationToken,
    max_bytes: u64,
    emit_progress: impl FnMut(u64, Option<u64>),
) -> Result<(), DownloadFailure> {
    download_to_disk_with_timeout(
        download_path,
        partial_path,
        final_path,
        cancel,
        max_bytes,
        emit_progress,
        MANAGED_DOWNLOAD_REQUEST_TIMEOUT,
    )
    .await
}

pub(super) async fn download_to_disk_with_timeout(
    download_path: &str,
    partial_path: &Path,
    final_path: &Path,
    cancel: &CancellationToken,
    max_bytes: u64,
    emit_progress: impl FnMut(u64, Option<u64>),
    request_timeout: Duration,
) -> Result<(), DownloadFailure> {
    let outcome = stream_to_disk(
        download_path,
        partial_path,
        final_path,
        cancel,
        max_bytes,
        emit_progress,
        request_timeout,
    )
    .await;
    let failure = match outcome {
        Ok(()) => return Ok(()),
        Err(failure) => failure,
    };
    match remove_partial_file(partial_path).await {
        Ok(()) => Err(failure),
        Err(()) => Err(DownloadFailure::Failed),
    }
}

async fn stream_to_disk(
    download_path: &str,
    partial_path: &Path,
    final_path: &Path,
    cancel: &CancellationToken,
    max_bytes: u64,
    mut emit_progress: impl FnMut(u64, Option<u64>),
    request_timeout: Duration,
) -> Result<(), DownloadFailure> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.url().scheme() == "https" && attempt.previous().len() < 10 {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .timeout(request_timeout)
        .build()
        .map_err(|_| DownloadFailure::Failed)?;
    let mut response = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(DownloadFailure::Cancelled),
        response = client.get(download_path).send() => response
            .map_err(|_| DownloadFailure::Failed)?,
    };
    let status = response.status();
    if !status.is_success() {
        return Err(DownloadFailure::Failed);
    }
    let total = response.content_length();
    if total.is_some_and(|bytes| bytes > max_bytes) {
        return Err(DownloadFailure::Failed);
    }
    let mut file = tokio::fs::File::create(partial_path)
        .await
        .map_err(|_| DownloadFailure::Failed)?;
    if cancel.is_cancelled() {
        return Err(DownloadFailure::Cancelled);
    }
    let mut downloaded = 0_u64;
    loop {
        let chunk = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(DownloadFailure::Cancelled),
            chunk = response.chunk() => chunk.map_err(|_| DownloadFailure::Failed)?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        let next_downloaded = downloaded
            .checked_add(chunk.len() as u64)
            .ok_or(DownloadFailure::Failed)?;
        if next_downloaded > max_bytes {
            return Err(DownloadFailure::Failed);
        }
        file.write_all(&chunk)
            .await
            .map_err(|_| DownloadFailure::Failed)?;
        if cancel.is_cancelled() {
            return Err(DownloadFailure::Cancelled);
        }
        downloaded = next_downloaded;
        emit_progress(downloaded, total);
    }
    file.flush().await.map_err(|_| DownloadFailure::Failed)?;
    if cancel.is_cancelled() {
        return Err(DownloadFailure::Cancelled);
    }
    file.sync_all().await.map_err(|_| DownloadFailure::Failed)?;
    if cancel.is_cancelled() {
        return Err(DownloadFailure::Cancelled);
    }
    drop(file);
    if cancel.is_cancelled() {
        return Err(DownloadFailure::Cancelled);
    }
    tokio::fs::rename(partial_path, final_path)
        .await
        .map_err(|_| DownloadFailure::Failed)?;
    Ok(())
}
