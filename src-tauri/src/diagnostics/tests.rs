use super::*;
use std::cell::Cell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver};

fn complete_snapshot() -> DiagnosticsStorageData {
    DiagnosticsStorageData {
        last_scan_display: "21 Mar 2026".to_string(),
        snapshot: Some(DiagnosticsSnapshotBody {
            local: LocalDiagnostics {
                passed: DiagnosticGroupStatus::Pass,
                model_path: ProviderValidation::Validated,
                model_file: ProviderValidation::Validated,
            },
            cloud: CloudDiagnostics {
                passed: DiagnosticGroupStatus::Pass,
                openai_api_key: ProviderValidation::Validated,
                openai_model: ProviderValidation::Validated,
                groq_api_key: ProviderValidation::NoneNotValid,
                groq_model: ProviderValidation::NoneNotValid,
            },
            microphone: MicrophoneDiagnostics {
                passed: DiagnosticGroupStatus::Pass,
                name: "Example Microphone".to_string(),
                specs: "48000 Hz, 2 Channels".to_string(),
            },
            transcription_access: TranscriptionAccessDiagnostics {
                passed: DiagnosticGroupStatus::Pass,
                function: TranscriptionFunction::SendInput,
                access_type: TranscriptionType::Cursor,
            },
        }),
    }
}

#[test]
fn date_formatting_is_stable() {
    assert_eq!(format_current_date().len(), 11);
}

#[test]
fn never_run_storage_is_neutral() {
    assert_eq!(DiagnosticsStorageData::default().last_scan_display, "");
    assert!(DiagnosticsStorageData::default().snapshot.is_none());
}

#[test]
fn completed_snapshot_round_trips_exactly() {
    let value = complete_snapshot();
    let json = serde_json::to_string(&value).unwrap();
    let decoded: DiagnosticsStorageData = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, value);
}

#[test]
fn serialized_contract_has_no_item_or_history_fields() {
    let json = serde_json::to_string(&complete_snapshot()).unwrap();
    for forbidden in [
        "\"results\"",
        "\"status\"",
        "\"message\"",
        "\"action\"",
        "\"history\"",
    ] {
        assert!(
            !json.contains(forbidden),
            "unexpected legacy/history field: {forbidden}"
        );
    }
    assert_eq!(json.matches("\"passed\"").count(), 4);
}

#[test]
fn local_diagnostics_contract_has_exactly_two_evaluation_items() {
    let local = complete_snapshot().snapshot.unwrap().local;
    let value = serde_json::to_value(local).unwrap();
    let object = value.as_object().unwrap();

    assert_eq!(
        object.len(),
        3,
        "Local Diagnostics is group status plus exactly two evaluation items"
    );
    assert!(object.contains_key("passed"));
    assert!(object.contains_key("model_path"));
    assert!(object.contains_key("model_file"));
    for forbidden in [
        "inference_backend",
        "vulkan_device",
        "gpu_type",
        "cpu_fallback",
    ] {
        assert!(
            !object.contains_key(forbidden),
            "forbidden Local Diagnostics field: {forbidden}"
        );
    }
}

#[test]
fn closed_value_domains_serialize_exactly() {
    assert_eq!(
        serde_json::to_string(&ProviderValidation::Validated).unwrap(),
        "\"validated\""
    );
    assert_eq!(
        serde_json::to_string(&ProviderValidation::NoneNotValid).unwrap(),
        "\"none_not_valid\""
    );
    assert_eq!(
        serde_json::to_string(&TranscriptionFunction::SendInput).unwrap(),
        "\"send_input\""
    );
    assert_eq!(
        serde_json::to_string(&TranscriptionFunction::Xdotool).unwrap(),
        "\"xdotool\""
    );
    assert_eq!(
        serde_json::to_string(&TranscriptionFunction::System).unwrap(),
        "\"system\""
    );
    assert_eq!(
        serde_json::to_string(&TranscriptionFunction::NoneNotValid).unwrap(),
        "\"none_not_valid\""
    );
    assert_eq!(
        serde_json::to_string(&TranscriptionType::Cursor).unwrap(),
        "\"cursor\""
    );
    assert_eq!(
        serde_json::to_string(&TranscriptionType::Clipboard).unwrap(),
        "\"clipboard\""
    );
    assert_eq!(
        serde_json::to_string(&TranscriptionType::NoneNotValid).unwrap(),
        "\"none_not_valid\""
    );
}

#[test]
fn old_item_schema_loads_as_never_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diagnostics.json");
    fs::write(
            &path,
            r#"{"last_scan_display":"21 Mar 2026","results":[{"check":"local_path","status":"validated","message":"Model Path: Validated","action":null}]}"#,
        )
        .unwrap();
    assert_eq!(load_storage(&path), DiagnosticsStorageData::default());
}

#[test]
fn atomic_persistence_round_trips_latest_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diagnostics.json");
    let data = complete_snapshot();
    save_storage(&path, &data).unwrap();
    assert_eq!(load_storage(&path), data);
}

#[test]
fn second_save_replaces_the_only_snapshot_and_date() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diagnostics.json");
    let first = complete_snapshot();
    save_storage(&path, &first).unwrap();

    let mut second = complete_snapshot();
    second.last_scan_display = "22 Mar 2026".to_string();
    second.snapshot.as_mut().unwrap().local = local_fail_unreachable();
    save_storage(&path, &second).unwrap();

    assert_eq!(load_storage(&path), second);
    let raw = fs::read_to_string(&path).unwrap();
    assert!(!raw.contains("21 Mar 2026"));
    assert_eq!(raw.matches("last_scan_display").count(), 1);
    assert!(!raw.contains("history"));
}

#[test]
fn incomplete_storage_is_rejected_without_creating_a_snapshot_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diagnostics.json");
    let incomplete = DiagnosticsStorageData::default();

    let error = save_storage(&path, &incomplete).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(!path.exists());
}

#[test]
fn failed_replacement_preserves_the_previous_persisted_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diagnostics.json");
    let first = complete_snapshot();
    save_storage(&path, &first).unwrap();

    let tmp = path.with_extension("json.tmp");
    fs::create_dir(&tmp).unwrap();
    let mut second = complete_snapshot();
    second.last_scan_display = "22 Mar 2026".to_string();

    assert!(save_storage(&path, &second).is_err());
    assert_eq!(load_storage(&path), first);
}

// Phase 3 orchestration ---------------------------------------------------

fn orchestration_local() -> LocalDiagnostics {
    LocalDiagnostics {
        passed: DiagnosticGroupStatus::Pass,
        model_path: ProviderValidation::Validated,
        model_file: ProviderValidation::Validated,
    }
}

fn orchestration_cloud() -> CloudDiagnostics {
    CloudDiagnostics {
        passed: DiagnosticGroupStatus::Pass,
        openai_api_key: ProviderValidation::Validated,
        openai_model: ProviderValidation::Validated,
        groq_api_key: ProviderValidation::NoneNotValid,
        groq_model: ProviderValidation::NoneNotValid,
    }
}

fn orchestration_microphone() -> MicrophoneDiagnostics {
    MicrophoneDiagnostics {
        passed: DiagnosticGroupStatus::Pass,
        name: "Test Mic".to_string(),
        specs: "48000 Hz, 2 Channels".to_string(),
    }
}

fn wait_for_release(gate: &Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>) {
    let (lock, condvar) = &**gate;
    let mut released = lock.lock().unwrap();
    while !*released {
        released = condvar.wait(released).unwrap();
    }
}

fn release_all(gate: &Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>) {
    let (lock, condvar) = &**gate;
    *lock.lock().unwrap() = true;
    condvar.notify_all();
}

#[test]
fn run_input_capture_reads_each_credential_once_and_freezes_settings() {
    struct CountingKeystore {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl KeystoreOps for CountingKeystore {
        fn get_key(&self, provider: ProviderId) -> Result<Option<String>, ()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Some(
                match provider {
                    ProviderId::Openai => "openai-key",
                    ProviderId::Groq => "groq-key",
                    ProviderId::Local => return Err(()),
                }
                .to_string(),
            ))
        }
    }

    let keystore = CountingKeystore {
        calls: std::sync::atomic::AtomicUsize::new(0),
    };
    let mut settings = AppSettings::default();
    settings.local_model_path = "C:/captured/model.bin".to_string();
    settings.openai_model = "captured-openai".to_string();
    settings.groq_model = "captured-groq".to_string();

    let captured = capture_run_inputs(
        &settings,
        &keystore,
        Some(PathBuf::from("C:/captured/vad.bin")),
    );
    settings.local_model_path = "C:/changed/model.bin".to_string();
    settings.openai_model = "changed-openai".to_string();
    settings.groq_model = "changed-groq".to_string();

    assert_eq!(keystore.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        captured.local_model_path,
        Some(PathBuf::from("C:/captured/model.bin"))
    );
    assert_eq!(
        captured.local_vad_model_path,
        Some(PathBuf::from("C:/captured/vad.bin"))
    );
    assert_eq!(captured.openai.model, "captured-openai");
    assert_eq!(captured.groq.model, "captured-groq");
    assert_eq!(
        captured.openai.key_result,
        Ok(Some("openai-key".to_string()))
    );
    assert_eq!(captured.groq.key_result, Ok(Some("groq-key".to_string())));
}

#[test]
fn all_four_top_level_groups_start_before_suite_can_finish() {
    let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let (started_tx, started_rx) = mpsc::channel::<&'static str>();
    let (result_tx, result_rx) = mpsc::channel();

    let runner_gate = Arc::clone(&gate);
    let handle = std::thread::spawn(move || {
        let local_gate = Arc::clone(&runner_gate);
        let cloud_gate = Arc::clone(&runner_gate);
        let mic_gate = Arc::clone(&runner_gate);
        let access_gate = Arc::clone(&runner_gate);
        let local_tx = started_tx.clone();
        let cloud_tx = started_tx.clone();
        let mic_tx = started_tx.clone();
        let access_tx = started_tx;

        let result = run_parallel_groups(
            move || {
                local_tx.send("local").unwrap();
                wait_for_release(&local_gate);
                orchestration_local()
            },
            move || {
                cloud_tx.send("cloud").unwrap();
                wait_for_release(&cloud_gate);
                Ok(orchestration_cloud())
            },
            move || {
                mic_tx.send("microphone").unwrap();
                wait_for_release(&mic_gate);
                orchestration_microphone()
            },
            move || {
                access_tx.send("access").unwrap();
                wait_for_release(&access_gate);
                access_pass(TranscriptionFunction::System, TranscriptionType::Clipboard)
            },
        );
        result_tx.send(result).unwrap();
    });

    let mut started = std::collections::HashSet::new();
    for _ in 0..4 {
        started.insert(started_rx.recv_timeout(Duration::from_secs(2)).unwrap());
    }
    assert_eq!(
        started,
        std::collections::HashSet::from(["local", "cloud", "microphone", "access"])
    );
    assert!(
        result_rx.try_recv().is_err(),
        "suite returned before all workers finished"
    );

    release_all(&gate);
    assert!(result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .is_ok());
    handle.join().unwrap();
}

#[test]
fn openai_and_groq_provider_branches_start_in_parallel() {
    let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let (started_tx, started_rx) = mpsc::channel::<&'static str>();
    let runner_gate = Arc::clone(&gate);

    let handle = std::thread::spawn(move || {
        let openai_gate = Arc::clone(&runner_gate);
        let groq_gate = Arc::clone(&runner_gate);
        let openai_tx = started_tx.clone();
        let groq_tx = started_tx;
        run_parallel_cloud(
            move || {
                openai_tx.send("openai").unwrap();
                wait_for_release(&openai_gate);
                (ProviderValidation::Validated, ProviderValidation::Validated)
            },
            move || {
                groq_tx.send("groq").unwrap();
                wait_for_release(&groq_gate);
                cloud_both_invalid()
            },
        )
    });

    let first = started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let second = started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_ne!(first, second);
    assert_eq!(
        std::collections::HashSet::from([first, second]),
        std::collections::HashSet::from(["openai", "groq"])
    );

    release_all(&gate);
    assert!(handle.join().unwrap().is_ok());
}

#[test]
fn cloud_provider_worker_failure_does_not_cancel_the_other_branch() {
    let groq_completed = Arc::new(AtomicBool::new(false));
    let completed = Arc::clone(&groq_completed);
    let result = run_parallel_cloud(
        || -> (ProviderValidation, ProviderValidation) {
            panic!("expected provider worker failure")
        },
        move || {
            std::thread::sleep(Duration::from_millis(25));
            completed.store(true, Ordering::SeqCst);
            cloud_both_invalid()
        },
    );

    assert_eq!(result, Err(DiagnosticsRunError::WorkerFailed));
    assert!(groq_completed.load(Ordering::SeqCst));
}

#[test]
fn multiple_cloud_provider_worker_failures_return_worker_failed() {
    let result = run_parallel_cloud(
        || -> (ProviderValidation, ProviderValidation) { panic!("expected OpenAI worker failure") },
        || -> (ProviderValidation, ProviderValidation) { panic!("expected Groq worker failure") },
    );

    assert_eq!(result, Err(DiagnosticsRunError::WorkerFailed));
}

#[test]
fn diagnostics_run_guard_rejects_duplicates_and_reopens_after_drop() {
    let guard = DiagnosticsRunGuard::default();
    let permit = guard.try_acquire().unwrap();
    assert!(matches!(
        guard.try_acquire(),
        Err(DiagnosticsRunError::AlreadyRunning)
    ));
    drop(permit);
    assert!(guard.try_acquire().is_ok());
}

#[test]
fn diagnostics_run_guard_reopens_after_unexpected_group_worker_failure() {
    let guard = DiagnosticsRunGuard::default();
    let result = {
        let _permit = guard.try_acquire().unwrap();
        run_parallel_groups(
            || -> LocalDiagnostics { panic!("expected group worker failure") },
            || Ok(orchestration_cloud()),
            orchestration_microphone,
            || access_pass(TranscriptionFunction::System, TranscriptionType::Clipboard),
        )
    };

    assert_eq!(result, Err(DiagnosticsRunError::WorkerFailed));
    assert!(guard.try_acquire().is_ok());
}

#[test]
fn multiple_top_level_worker_failures_return_worker_failed() {
    let result = run_parallel_groups(
        || -> LocalDiagnostics { panic!("expected Local worker failure") },
        || Ok(orchestration_cloud()),
        || -> MicrophoneDiagnostics { panic!("expected Microphone worker failure") },
        || access_pass(TranscriptionFunction::System, TranscriptionType::Clipboard),
    );

    assert_eq!(result, Err(DiagnosticsRunError::WorkerFailed));
}

#[test]
fn run_on_demand_builds_one_complete_snapshot_after_group_join() {
    let data = run_on_demand(DiagnosticsRunInputs {
        local_model_path: None,
        local_vad_model_path: None,
        openai: CloudProviderRunInput {
            key_result: Ok(None),
            model: "unused-openai".to_string(),
        },
        groq: CloudProviderRunInput {
            key_result: Ok(None),
            model: "unused-groq".to_string(),
        },
    })
    .unwrap();

    assert!(data.snapshot.is_some());
    assert!(!data.last_scan_display.is_empty());
}

// Local ------------------------------------------------------------------

fn diagnostics_vad_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(crate::transcription::local::BUNDLED_VAD_RESOURCE_PATH)
}

#[test]
fn diagnostic_speech_fixture_is_embedded_and_decodable() {
    let (samples, sample_rate) =
        diagnostic_speech_input().expect("embedded speech fixture must decode");
    assert!(!samples.is_empty());
    assert!(sample_rate > 0);
    assert!(samples.iter().any(|sample| sample.abs() > 0.001));
}

#[test]
fn local_no_selection_fails_both_lines() {
    let local = test_local_provider_path(
        resolve_local_model_path(&AppSettings::default()),
        Some(diagnostics_vad_model_path()),
    );
    assert_eq!(local, local_fail_unreachable());
}

#[test]
fn local_model_path_uses_selected_path_regardless_of_model_kind_metadata() {
    let mut settings = AppSettings::default();
    settings.local_model_path = "C:/selected/model.bin".to_string();

    for kind in [
        None,
        Some(crate::models::LocalModelKind::Standard),
        Some(crate::models::LocalModelKind::Custom),
    ] {
        settings.local_model_kind = kind;
        assert_eq!(
            resolve_local_model_path(&settings),
            Some(PathBuf::from("C:/selected/model.bin"))
        );
    }
}

#[test]
fn local_unreachable_selected_path_fails_both_lines_regardless_of_model_kind() {
    for kind in [
        Some(crate::models::LocalModelKind::Standard),
        Some(crate::models::LocalModelKind::Custom),
    ] {
        let mut settings = AppSettings::default();
        settings.local_model_kind = kind;
        settings.local_model_path = "/definitely/missing/model.bin".to_string();
        assert_eq!(
            test_local_provider_path(
                resolve_local_model_path(&settings),
                Some(diagnostics_vad_model_path()),
            ),
            local_fail_unreachable()
        );
    }
}

#[test]
fn local_missing_vad_preserves_reachable_model_path_fact() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reachable-model.bin");
    fs::write(&path, b"reachable model artifact").unwrap();

    let local = test_local_provider_path(Some(path), None);
    assert_eq!(local.passed, DiagnosticGroupStatus::Fail);
    assert_eq!(local.model_path, ProviderValidation::Validated);
    assert_eq!(local.model_file, ProviderValidation::NoneNotValid);
}

#[test]
fn local_invalid_model_validates_path_but_fails_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("garbage.bin");
    fs::write(&path, b"not a whisper model").unwrap();
    let mut settings = AppSettings::default();
    settings.local_model_path = path.to_string_lossy().to_string();

    let local = test_local_provider_path(
        resolve_local_model_path(&settings),
        Some(diagnostics_vad_model_path()),
    );
    assert_eq!(local.passed, DiagnosticGroupStatus::Fail);
    assert_eq!(local.model_path, ProviderValidation::Validated);
    assert_eq!(local.model_file, ProviderValidation::NoneNotValid);
}

#[test]
fn local_diagnostics_do_not_touch_a_production_local_engine_cache() {
    let engine = crate::transcription::local::LocalEngine::new();
    assert!(engine.cached_path().is_none());

    let mut settings = AppSettings::default();
    settings.local_model_path = "/definitely/missing/model.bin".to_string();
    let _ = test_local_provider_path(
        resolve_local_model_path(&settings),
        Some(diagnostics_vad_model_path()),
    );

    assert!(engine.cached_path().is_none());
}
// Cloud ------------------------------------------------------------------

#[test]
fn cloud_missing_key_and_key_read_failure_make_no_request() {
    for key_result in [Ok(None), Err(())] {
        let calls = Cell::new(0);
        let result = evaluate_cloud_provider(key_result, "model", |_, _| {
            calls.set(calls.get() + 1);
            CloudProbeOutcome::HttpStatus(200)
        });
        assert_eq!(result, cloud_both_invalid());
        assert_eq!(calls.get(), 0);
    }
}

#[test]
fn cloud_missing_model_makes_no_request() {
    let calls = Cell::new(0);
    let result = evaluate_cloud_provider(Ok(Some("key".to_string())), "", |_, _| {
        calls.set(calls.get() + 1);
        CloudProbeOutcome::HttpStatus(200)
    });
    assert_eq!(result, cloud_both_invalid());
    assert_eq!(calls.get(), 0);
}

#[test]
fn cloud_status_rules_are_provider_independent() {
    let cases = [
        (
            CloudProbeOutcome::HttpStatus(200),
            (ProviderValidation::Validated, ProviderValidation::Validated),
        ),
        (CloudProbeOutcome::HttpStatus(401), cloud_both_invalid()),
        (CloudProbeOutcome::HttpStatus(403), cloud_both_invalid()),
        (
            CloudProbeOutcome::HttpStatus(400),
            (
                ProviderValidation::Validated,
                ProviderValidation::NoneNotValid,
            ),
        ),
        (
            CloudProbeOutcome::HttpStatus(429),
            (
                ProviderValidation::Validated,
                ProviderValidation::NoneNotValid,
            ),
        ),
        (
            CloudProbeOutcome::HttpStatus(500),
            (
                ProviderValidation::Validated,
                ProviderValidation::NoneNotValid,
            ),
        ),
        (CloudProbeOutcome::NoResponse, cloud_both_invalid()),
    ];

    for provider in [ProviderId::Openai, ProviderId::Groq] {
        for (outcome, expected) in cases {
            let result =
                evaluate_cloud_provider(Ok(Some("key".to_string())), "model", |_, _| outcome);
            assert_eq!(
                result, expected,
                "provider={provider:?} outcome={outcome:?}"
            );
        }
    }
}

#[test]
fn cloud_group_passes_when_either_provider_passes() {
    assert!(provider_passes(
        ProviderValidation::Validated,
        ProviderValidation::Validated
    ));
    assert!(!provider_passes(
        ProviderValidation::Validated,
        ProviderValidation::NoneNotValid
    ));
}

fn serve_once(status: u16, body: &str) -> (String, Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let body = body.to_string();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        let request_text = String::from_utf8_lossy(&request);
                        let content_length = request_text
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        let header_end = request
                            .windows(4)
                            .position(|window| window == b"\r\n\r\n")
                            .map(|index| index + 4)
                            .unwrap_or(request.len());
                        if request.len() >= header_end + content_length {
                            break;
                        }
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(request);
        let response = format!(
                "HTTP/1.1 {status} Test\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    });
    (format!("http://{address}/audio/transcriptions"), rx)
}

#[test]
fn direct_cloud_probe_sends_one_multipart_request_per_provider() {
    let wav = encode_wav_16bit_mono(&vec![0.0f32; 1600]).unwrap();
    for provider in [ProviderId::Openai, ProviderId::Groq] {
        let (url, request_rx) = serve_once(200, "ignored body");
        let spec = cloud_probe_spec(provider).unwrap();
        let outcome = send_cloud_probe_to(spec, &url, "test-key", "test-model", &wav);
        assert_eq!(outcome, CloudProbeOutcome::HttpStatus(200));

        let request = String::from_utf8_lossy(&request_rx.recv().unwrap()).to_string();
        assert!(request.contains("test-model"));
        assert!(request.contains(spec.response_format));
        assert_eq!(request.matches("test-model").count(), 1);
    }
}

#[test]
fn cloud_response_body_text_has_zero_effect_on_classification() {
    let (url, _) = serve_once(500, "401 Unauthorized invalid_api_key model not found");
    let wav = encode_wav_16bit_mono(&vec![0.0f32; 1600]).unwrap();
    let spec = cloud_probe_spec(ProviderId::Openai).unwrap();
    let outcome = send_cloud_probe_to(spec, &url, "key", "model", &wav);
    assert_eq!(outcome, CloudProbeOutcome::HttpStatus(500));
    assert_eq!(
        classify_cloud_probe(outcome),
        (
            ProviderValidation::Validated,
            ProviderValidation::NoneNotValid,
        )
    );
}

#[test]
fn cloud_transport_failure_invalidates_both_values() {
    let spec = cloud_probe_spec(ProviderId::Openai).unwrap();
    let wav = encode_wav_16bit_mono(&vec![0.0f32; 1600]).unwrap();
    let outcome = send_cloud_probe_to(spec, "http://127.0.0.1:1/unreachable", "key", "model", &wav);
    assert_eq!(outcome, CloudProbeOutcome::NoResponse);
    assert_eq!(classify_cloud_probe(outcome), cloud_both_invalid());
}

fn mock_live_ws_probe_server(
    response_events: Vec<&'static str>,
    delay: Option<Duration>,
) -> (String, tokio::sync::mpsc::Receiver<String>) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::accept_hdr_async;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
    use tokio_tungstenite::tungstenite::Message;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (received_tx, received_rx) = tokio::sync::mpsc::channel(10);

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            if let Ok((stream, _)) = listener.accept().await {
                let callback = |_req: &Request, response: Response| Ok(response);
                if let Ok(ws_stream) = accept_hdr_async(stream, callback).await {
                    let (mut ws_writer, mut ws_reader) = ws_stream.split();
                    if let Some(Ok(Message::Text(text))) = ws_reader.next().await {
                        let _ = received_tx.send(text.to_string()).await;
                        if let Some(delay_dur) = delay {
                            tokio::time::sleep(delay_dur).await;
                        }
                        for event in response_events {
                            let _ = ws_writer.send(Message::Text(event.to_string().into())).await;
                        }
                    }
                }
            }
        });
    });

    (format!("ws://127.0.0.1:{port}"), received_rx)
}

#[test]
fn direct_openai_live_probe_valid_session_passes_both() {
    let (ws_url, mut received_rx) = mock_live_ws_probe_server(
        vec![r#"{"type":"session.created"}"#, r#"{"type":"session.updated"}"#],
        None,
    );
    let outcome = send_openai_live_probe_to(&ws_url, "test-api-key", "gpt-live-transcribe");
    assert_eq!(outcome, CloudProbeOutcome::HttpStatus(200));
    assert_eq!(
        classify_cloud_probe(outcome),
        (ProviderValidation::Validated, ProviderValidation::Validated)
    );
    let received = received_rx.blocking_recv().unwrap();
    assert!(received.contains("gpt-live-transcribe"));
}

#[test]
fn direct_openai_live_probe_model_error_event_validates_key_only() {
    let (ws_url, _) = mock_live_ws_probe_server(
        vec![r#"{"type":"session.created"}"#, r#"{"type":"error","error":{"code":"missing_model","message":"invalid model"}}"#],
        None,
    );
    let outcome = send_openai_live_probe_to(&ws_url, "test-api-key", "invalid-model");
    assert_eq!(outcome, CloudProbeOutcome::HttpStatus(400));
    assert_eq!(
        classify_cloud_probe(outcome),
        (ProviderValidation::Validated, ProviderValidation::NoneNotValid)
    );
}

#[test]
fn direct_openai_live_probe_auth_error_event_invalidates_both() {
    let (ws_url, _) = mock_live_ws_probe_server(
        vec![r#"{"type":"error","error":{"code":"invalid_api_key","message":"unauthorized"}}"#],
        None,
    );
    let outcome = send_openai_live_probe_to(&ws_url, "invalid-key", "gpt-live-transcribe");
    assert_eq!(outcome, CloudProbeOutcome::HttpStatus(401));
    assert_eq!(
        classify_cloud_probe(outcome),
        (ProviderValidation::NoneNotValid, ProviderValidation::NoneNotValid)
    );
}

#[test]
fn direct_openai_live_probe_unreachable_returns_no_response() {
    let outcome = send_openai_live_probe_to("ws://127.0.0.1:1/unreachable", "key", "gpt-live-transcribe");
    assert_eq!(outcome, CloudProbeOutcome::NoResponse);
    assert_eq!(classify_cloud_probe(outcome), cloud_both_invalid());
}

// Microphone -------------------------------------------------------------

#[test]
fn microphone_no_device_or_name_failure_invalidates_both_lines() {
    let result = classify_microphone(None, Some((48_000, 2)));
    assert_eq!(result.passed, DiagnosticGroupStatus::Fail);
    assert_eq!(result.name, NONE_NOT_VALID);
    assert_eq!(result.specs, NONE_NOT_VALID);
}

#[test]
fn microphone_config_failure_preserves_real_name_only() {
    let result = classify_microphone(Some("USB Mic".to_string()), None);
    assert_eq!(result.passed, DiagnosticGroupStatus::Fail);
    assert_eq!(result.name, "USB Mic");
    assert_eq!(result.specs, NONE_NOT_VALID);
}

#[test]
fn microphone_accepts_arbitrary_name_and_formats_specs() {
    let result = classify_microphone(
        Some("A very specific OS microphone name".to_string()),
        Some((48_000, 2)),
    );
    assert_eq!(result.passed, DiagnosticGroupStatus::Pass);
    assert_eq!(result.name, "A very specific OS microphone name");
    assert_eq!(result.specs, "48000 Hz, 2 Channels");
}

#[test]
fn microphone_zero_rate_or_channels_is_not_usable() {
    for specs in [Some((0, 2)), Some((48_000, 0))] {
        let result = classify_microphone(Some("Mic".to_string()), specs);
        assert_eq!(result.passed, DiagnosticGroupStatus::Fail);
        assert_eq!(result.specs, NONE_NOT_VALID);
    }
}

#[test]
fn microphone_inspection_timeout_policy_is_five_seconds() {
    assert_eq!(MICROPHONE_INSPECTION_TIMEOUT, Duration::from_secs(5));
}

// Transcription Access ---------------------------------------------------

#[cfg(target_os = "windows")]
#[test]
fn current_windows_transcription_access_reports_sendinput_cursor() {
    assert_eq!(
        test_transcription_access(),
        access_pass(TranscriptionFunction::SendInput, TranscriptionType::Cursor)
    );
}

#[test]
fn clipboard_validation_timeout_policy_is_three_seconds() {
    assert_eq!(CLIPBOARD_VALIDATION_TIMEOUT, Duration::from_secs(3));
}

#[test]
fn access_selected_sendinput_reports_cursor_without_probing_clipboard() {
    let clipboard_probes = std::sync::atomic::AtomicUsize::new(0);
    let result = classify_transcription_access(DeliveryMethod::SendInput, || {
        clipboard_probes.fetch_add(1, Ordering::SeqCst);
        true
    });
    assert_eq!(
        result,
        access_pass(TranscriptionFunction::SendInput, TranscriptionType::Cursor)
    );
    assert_eq!(clipboard_probes.load(Ordering::SeqCst), 0);
}

#[test]
fn access_selected_xdotool_reports_cursor_without_probing_clipboard() {
    let clipboard_probes = std::sync::atomic::AtomicUsize::new(0);
    let result = classify_transcription_access(DeliveryMethod::Xdotool, || {
        clipboard_probes.fetch_add(1, Ordering::SeqCst);
        true
    });
    assert_eq!(
        result,
        access_pass(TranscriptionFunction::Xdotool, TranscriptionType::Cursor)
    );
    assert_eq!(clipboard_probes.load(Ordering::SeqCst), 0);
}

#[test]
fn access_selected_clipboard_probes_and_reports_system_when_available() {
    let clipboard_probes = std::sync::atomic::AtomicUsize::new(0);
    let result = classify_transcription_access(DeliveryMethod::Clipboard, || {
        clipboard_probes.fetch_add(1, Ordering::SeqCst);
        true
    });
    assert_eq!(
        result,
        access_pass(TranscriptionFunction::System, TranscriptionType::Clipboard)
    );
    assert_eq!(clipboard_probes.load(Ordering::SeqCst), 1);
}

#[test]
fn access_selected_clipboard_probes_and_fails_when_unavailable() {
    let clipboard_probes = std::sync::atomic::AtomicUsize::new(0);
    let result = classify_transcription_access(DeliveryMethod::Clipboard, || {
        clipboard_probes.fetch_add(1, Ordering::SeqCst);
        false
    });
    assert_eq!(result, access_fail());
    assert_eq!(clipboard_probes.load(Ordering::SeqCst), 1);
}

#[test]
fn diagnostics_snapshot_derives_capability_from_approved_function_type_pairs() {
    let mut data = complete_snapshot();

    data.snapshot.as_mut().unwrap().transcription_access =
        access_pass(TranscriptionFunction::SendInput, TranscriptionType::Cursor);
    assert_eq!(
        delivery_capability_from_snapshot(&data),
        DeliveryCapability::Inject
    );

    data.snapshot.as_mut().unwrap().transcription_access =
        access_pass(TranscriptionFunction::Xdotool, TranscriptionType::Cursor);
    assert_eq!(
        delivery_capability_from_snapshot(&data),
        DeliveryCapability::Inject
    );

    data.snapshot.as_mut().unwrap().transcription_access =
        access_pass(TranscriptionFunction::System, TranscriptionType::Clipboard);
    assert_eq!(
        delivery_capability_from_snapshot(&data),
        DeliveryCapability::Clipboard
    );

    data.snapshot.as_mut().unwrap().transcription_access = access_fail();
    assert_eq!(
        delivery_capability_from_snapshot(&data),
        DeliveryCapability::Clipboard
    );
}

#[test]
fn inconsistent_access_snapshot_fails_closed_to_clipboard() {
    let mut data = complete_snapshot();
    data.snapshot.as_mut().unwrap().transcription_access =
        access_pass(TranscriptionFunction::System, TranscriptionType::Cursor);
    assert_eq!(
        delivery_capability_from_snapshot(&data),
        DeliveryCapability::Clipboard
    );
}
