use super::*;
use std::io::{Read, Write};

const SMALL_PATH: &str =
    "https://models.example.test/whisper/environment-alpha.bin?source=environment";
const MEDIUM_PATH: &str = "https://another.example.test/models/environment-beta.bin";

fn manager() -> (tempfile::TempDir, DownloadManager) {
    let dir = tempfile::tempdir().unwrap();
    let models = dir.path().join("models");
    (
        dir,
        DownloadManager::new(
            models,
            vec![SMALL_PATH.to_string(), MEDIUM_PATH.to_string()],
        )
        .unwrap(),
    )
}

fn download_paths(dir: &tempfile::TempDir) -> (PathBuf, PathBuf) {
    (
        dir.path().join("model.partial"),
        dir.path().join("model.bin"),
    )
}

#[test]
fn managed_model_path_validation_accepts_only_configured_destinations() {
    let (_dir, manager) = manager();
    assert!(manager.is_managed_model_path(&manager.model_path(SMALL_PATH)));
    assert!(manager.is_managed_model_path(&manager.model_path(MEDIUM_PATH)));
    assert!(!manager
        .is_managed_model_path(&manager.models_dir().join("renderer-supplied-standard.bin")));
}

#[test]
fn selected_deletion_recovery_journal_round_trips_exact_managed_path() {
    let (_dir, manager) = manager();
    let path = manager.model_path(SMALL_PATH);

    begin_selected_deletion_recovery(manager.models_dir(), &path).unwrap();
    assert_eq!(
        pending_selected_deletion_recovery(manager.models_dir()).unwrap(),
        Some(path)
    );

    clear_selected_deletion_recovery(manager.models_dir()).unwrap();
    assert_eq!(
        pending_selected_deletion_recovery(manager.models_dir()).unwrap(),
        None
    );
}

#[test]
fn managed_download_request_timeout_policy_is_thirty_minutes() {
    assert_eq!(
        transfer::MANAGED_DOWNLOAD_REQUEST_TIMEOUT,
        Duration::from_secs(30 * 60)
    );
}

#[test]
fn storage_identity_is_stable_and_path_specific() {
    let small = DownloadManager::storage_file_name(SMALL_PATH);
    assert_eq!(small, DownloadManager::storage_file_name(SMALL_PATH));
    assert_ne!(small, DownloadManager::storage_file_name(MEDIUM_PATH));
    assert!(small.ends_with(".bin"));
}

#[test]
fn source_text_cannot_escape_models_directory() {
    let (_dir, manager) = manager();
    let path = manager.model_path("../../outside.bin");
    assert_eq!(path.parent(), Some(manager.models_dir()));
    assert!(!path.file_name().unwrap().to_string_lossy().contains(".."));
}

#[test]
fn list_reports_disk_state_by_download_path() {
    let (_dir, manager) = manager();
    std::fs::create_dir_all(manager.models_dir()).unwrap();
    std::fs::write(manager.model_path(SMALL_PATH), b"12345").unwrap();
    let infos = manager.list();
    assert_eq!(infos.len(), 2);
    assert_eq!(infos[0].download_path, SMALL_PATH);
    assert!(infos[0].downloaded);
    assert_eq!(infos[0].size_bytes, Some(5));
    assert_eq!(infos[1].download_path, MEDIUM_PATH);
    assert!(!infos[1].downloaded);
    assert_eq!(infos[1].size_bytes, None);
}

#[test]
fn manager_rejects_non_https_duplicate_and_excess_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let models = dir.path().join("models");
    assert!(DownloadManager::new(
        models.clone(),
        vec!["http://models.example.test/model.bin".to_string()]
    )
    .is_err());
    assert!(DownloadManager::new(
        models.clone(),
        vec![SMALL_PATH.to_string(), SMALL_PATH.to_string()]
    )
    .is_err());
    assert!(DownloadManager::new(
        models,
        vec![
            SMALL_PATH.to_string(),
            MEDIUM_PATH.to_string(),
            "https://third.example.test/model.bin".to_string(),
        ]
    )
    .is_err());
}

#[test]
fn manager_rejects_every_operation_for_an_unconfigured_path() {
    let (_dir, manager) = manager();
    let unknown = "https://unconfigured.example.test/model.bin";
    assert!(manager.cancel(unknown).is_err());
    assert!(manager.delete(unknown).is_err());
}

#[test]
fn download_budget_enforces_per_file_and_directory_limits() {
    assert_eq!(
        DownloadManager::download_budget(0).unwrap(),
        MAX_DOWNLOAD_BYTES
    );
    assert_eq!(
        DownloadManager::download_budget(MAX_MODELS_DIR_BYTES - 7).unwrap(),
        7
    );
    assert!(DownloadManager::download_budget(MAX_MODELS_DIR_BYTES).is_err());
    assert!(DownloadManager::download_budget(MAX_MODELS_DIR_BYTES + 1).is_err());
}

#[tokio::test]
async fn downloader_requests_exact_path_and_reports_known_total() {
    use std::net::TcpListener;
    use std::sync::mpsc;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (request_tx, request_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let count = stream.read(&mut request).unwrap();
        request_tx
            .send(String::from_utf8_lossy(&request[..count]).to_string())
            .unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nmodel")
            .unwrap();
    });
    let download_path = format!("http://{address}/exact/environment/model.bin?token=unchanged");
    let dir = tempfile::tempdir().unwrap();
    let (partial_path, final_path) = download_paths(&dir);
    let cancel = CancellationToken::new();
    let mut progress = Vec::new();

    download_to_disk(
        &download_path,
        &partial_path,
        &final_path,
        &cancel,
        MAX_DOWNLOAD_BYTES,
        |downloaded, total| progress.push((downloaded, total)),
    )
    .await
    .unwrap();
    server.join().unwrap();

    let request = request_rx.recv().unwrap();
    assert!(
        request.starts_with("GET /exact/environment/model.bin?token=unchanged HTTP/1.1"),
        "unexpected request: {request}"
    );
    assert_eq!(progress.last(), Some(&(5, Some(5))));
    assert_eq!(std::fs::read(final_path).unwrap(), b"model");
    assert!(!partial_path.exists());
}

#[tokio::test]
async fn unknown_content_length_stays_unknown() {
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        stream.read(&mut request).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nmodel")
            .unwrap();
    });
    let download_path = format!("http://{address}/model.bin");
    let dir = tempfile::tempdir().unwrap();
    let (partial_path, final_path) = download_paths(&dir);
    let cancel = CancellationToken::new();
    let mut progress = Vec::new();

    download_to_disk(
        &download_path,
        &partial_path,
        &final_path,
        &cancel,
        MAX_DOWNLOAD_BYTES,
        |downloaded, total| progress.push((downloaded, total)),
    )
    .await
    .unwrap();
    server.join().unwrap();

    assert_eq!(progress.last(), Some(&(5, None)));
    assert_eq!(std::fs::read(final_path).unwrap(), b"model");
    assert!(!partial_path.exists());
}

#[tokio::test]
async fn declared_size_over_limit_is_rejected_before_file_creation() {
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        stream.read(&mut request).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n")
            .unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let (partial_path, final_path) = download_paths(&dir);

    let error = download_to_disk(
        &format!("http://{address}/oversized.bin"),
        &partial_path,
        &final_path,
        &CancellationToken::new(),
        5,
        |_, _| {},
    )
    .await
    .unwrap_err();
    server.join().unwrap();

    assert_eq!(error, DownloadFailure::Failed);
    assert!(!partial_path.exists());
    assert!(!final_path.exists());
}

#[tokio::test]
async fn streamed_size_over_limit_removes_partial_file() {
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        stream.read(&mut request).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nmodel!")
            .unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let (partial_path, final_path) = download_paths(&dir);

    let error = download_to_disk(
        &format!("http://{address}/streamed-oversized.bin"),
        &partial_path,
        &final_path,
        &CancellationToken::new(),
        5,
        |_, _| {},
    )
    .await
    .unwrap_err();
    server.join().unwrap();

    assert_eq!(error, DownloadFailure::Failed);
    assert!(!partial_path.exists());
    assert!(!final_path.exists());
}

#[tokio::test]
async fn request_timeout_interrupts_stalled_read_and_removes_partial_file() {
    use std::net::TcpListener;
    use std::sync::mpsc;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (headers_tx, headers_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        stream.read(&mut request).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        headers_tx.send(()).unwrap();
        let _ = release_rx.recv_timeout(Duration::from_secs(5));
    });
    let download_path = format!("http://{address}/timeout-model.bin");
    let dir = tempfile::tempdir().unwrap();
    let (partial_path, final_path) = download_paths(&dir);
    let task_partial = partial_path.clone();
    let task_final = final_path.clone();
    let task = tokio::spawn(async move {
        transfer::download_to_disk_with_timeout(
            &download_path,
            &task_partial,
            &task_final,
            &CancellationToken::new(),
            MAX_DOWNLOAD_BYTES,
            |_, _| {},
            Duration::from_millis(100),
        )
        .await
    });

    headers_rx.await.unwrap();
    let outcome = task.await.unwrap();
    release_tx.send(()).unwrap();
    server.join().unwrap();

    assert_eq!(outcome, Err(DownloadFailure::Failed));
    assert!(!partial_path.exists());
    assert!(!final_path.exists());
}

#[tokio::test]
async fn cancellation_interrupts_stalled_read_and_removes_partial_file() {
    use std::net::TcpListener;
    use std::sync::mpsc;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (headers_tx, headers_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        stream.read(&mut request).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        headers_tx.send(()).unwrap();
        let _ = release_rx.recv_timeout(Duration::from_secs(5));
    });
    let download_path = format!("http://{address}/stalled-model.bin");
    let dir = tempfile::tempdir().unwrap();
    let (partial_path, final_path) = download_paths(&dir);
    let task_partial = partial_path.clone();
    let task_final = final_path.clone();
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        download_to_disk(
            &download_path,
            &task_partial,
            &task_final,
            &task_cancel,
            MAX_DOWNLOAD_BYTES,
            |_, _| {},
        )
        .await
    });

    headers_rx.await.unwrap();
    cancel.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("cancellation did not interrupt the stalled response")
        .unwrap();
    release_tx.send(()).unwrap();
    server.join().unwrap();

    assert_eq!(outcome, Err(DownloadFailure::Cancelled));
    assert!(!partial_path.exists());
    assert!(!final_path.exists());
}

#[tokio::test]
async fn invalid_download_path_fails_without_creating_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let (partial_path, final_path) = download_paths(&dir);
    let cancel = CancellationToken::new();

    let error = download_to_disk(
        "not a valid download path",
        &partial_path,
        &final_path,
        &cancel,
        MAX_DOWNLOAD_BYTES,
        |_, _| {},
    )
    .await
    .unwrap_err();

    assert_eq!(error, DownloadFailure::Failed);
    assert!(!partial_path.exists());
    assert!(!final_path.exists());
}

#[tokio::test]
async fn partial_cleanup_failure_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let (partial_path, final_path) = download_paths(&dir);
    std::fs::create_dir(&partial_path).unwrap();
    let cancel = CancellationToken::new();

    let error = download_to_disk(
        "not a valid download path",
        &partial_path,
        &final_path,
        &cancel,
        MAX_DOWNLOAD_BYTES,
        |_, _| {},
    )
    .await
    .unwrap_err();

    assert_eq!(error, DownloadFailure::Failed);
    assert!(partial_path.is_dir());
    assert!(!final_path.exists());
}
#[test]
fn stale_partial_for_requested_model_is_removed_before_restart_budget() {
    let (_dir, manager) = manager();
    std::fs::create_dir_all(manager.models_dir()).unwrap();

    let installed = manager.model_path(MEDIUM_PATH);
    let stale_partial = manager.partial_path(SMALL_PATH);
    std::fs::File::create(&installed)
        .unwrap()
        .set_len(MAX_DOWNLOAD_BYTES)
        .unwrap();
    std::fs::File::create(&stale_partial)
        .unwrap()
        .set_len(MAX_DOWNLOAD_BYTES)
        .unwrap();
    assert!(manager.available_download_bytes().is_err());

    let (_final_path, partial_path, cancel, max_bytes) = manager.prepare_start(SMALL_PATH).unwrap();

    assert_eq!(partial_path, stale_partial);
    assert!(!stale_partial.exists());
    assert_eq!(max_bytes, MAX_DOWNLOAD_BYTES);
    assert!(!cancel.is_cancelled());
    assert!(manager.is_downloading());
    manager.finish(SMALL_PATH);
}

#[test]
fn busy_start_does_not_remove_another_paths_partial() {
    let (_dir, manager) = manager();
    std::fs::create_dir_all(manager.models_dir()).unwrap();
    let other_partial = manager.partial_path(MEDIUM_PATH);
    std::fs::write(
        &other_partial,
        b"preserve while another download owns the slot",
    )
    .unwrap();
    manager.acquire_slot(SMALL_PATH).unwrap();

    assert_eq!(
        manager.prepare_start(MEDIUM_PATH).unwrap_err(),
        DownloadError::Busy
    );
    assert!(other_partial.is_file());
    manager.finish(SMALL_PATH);
}

#[test]
fn single_download_slot_is_enforced() {
    let (_dir, manager) = manager();
    let cancel = manager.acquire_slot(SMALL_PATH).unwrap();
    assert!(manager.is_downloading());
    assert_eq!(
        manager.acquire_slot(MEDIUM_PATH).unwrap_err(),
        DownloadError::Busy
    );
    assert!(!cancel.is_cancelled());
    manager.finish(SMALL_PATH);
    assert!(!manager.is_downloading());
    manager.acquire_slot(MEDIUM_PATH).unwrap();
}

#[test]
fn cancel_only_matches_exact_active_path() {
    let (_dir, manager) = manager();
    manager.acquire_slot(SMALL_PATH).unwrap();
    assert!(manager.cancel(MEDIUM_PATH).is_err());
    manager.cancel(SMALL_PATH).unwrap();
    let guard = manager.active.lock();
    assert!(guard.as_ref().unwrap().cancel.is_cancelled());
}

#[test]
fn delete_removes_path_derived_file_and_stale_partial() {
    let (_dir, manager) = manager();
    std::fs::create_dir_all(manager.models_dir()).unwrap();
    let path = manager.model_path(SMALL_PATH);
    let partial_path = manager.partial_path(SMALL_PATH);
    std::fs::write(&path, b"complete").unwrap();
    std::fs::write(&partial_path, b"partial").unwrap();

    manager.delete(SMALL_PATH).unwrap();

    assert!(!path.exists());
    assert!(!partial_path.exists());

    std::fs::write(&partial_path, b"stale").unwrap();
    assert!(manager.delete(SMALL_PATH).is_err());
    assert!(!partial_path.exists());
}

#[test]
fn delete_rejects_an_active_download_of_the_same_path() {
    let (_dir, manager) = manager();
    std::fs::create_dir_all(manager.models_dir()).unwrap();
    let path = manager.model_path(SMALL_PATH);
    let partial_path = manager.partial_path(SMALL_PATH);
    std::fs::write(&path, b"complete").unwrap();
    std::fs::write(&partial_path, b"partial").unwrap();
    manager.acquire_slot(SMALL_PATH).unwrap();

    assert_eq!(
        manager.delete(SMALL_PATH).unwrap_err(),
        DownloadError::DeleteWhileActive
    );
    assert!(path.exists());
    assert!(partial_path.exists());
    manager.finish(SMALL_PATH);
}
