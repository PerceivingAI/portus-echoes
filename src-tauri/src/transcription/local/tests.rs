use super::*;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use whisper_rs::vulkan::{VulkanDeviceInfo, VulkanDeviceKind};

use super::feed::build_test_audio_feed;
use super::inference::{transcribe_with_context, LOCAL_INFERENCE_TIMEOUT};
use super::vad::VadAssetError;

#[test]
fn automatic_language_policy_preserves_literal_auto() {
    let policy = LocalLanguagePolicy::from_build_time("auto");
    assert_eq!(policy, LocalLanguagePolicy::AutoPerSegment);
    assert_eq!(policy.language(), Some("auto"));
}

#[test]
fn fixed_language_policy_preserves_compiled_code() {
    let policy = LocalLanguagePolicy::from_build_time("en");
    assert_eq!(policy, LocalLanguagePolicy::Fixed(LanguageCode::new("en")));
    assert_eq!(policy.language(), Some("en"));
}

#[test]
fn local_recording_config_uses_shared_delivery_capability() {
    let settings = AppSettings::default();
    let inject = LocalRecordingConfig::from_settings(&settings, DeliveryCapability::Inject);
    let clipboard = LocalRecordingConfig::from_settings(&settings, DeliveryCapability::Clipboard);
    assert_eq!(inject.output, LocalOutputPolicy::LiveInject);
    assert_eq!(clipboard.output, LocalOutputPolicy::TerminalClipboard);
}

#[test]
fn local_recording_config_uses_local_build_time_language_contract() {
    let settings = AppSettings::default();
    let config = LocalRecordingConfig::from_settings(&settings, DeliveryCapability::Inject);
    let expected = LocalLanguagePolicy::from_build_time(env!("PORTUS_LOCAL_LANGUAGE"));
    assert_eq!(config.language, expected);
    assert_eq!(
        config.language.language(),
        Some(env!("PORTUS_LOCAL_LANGUAGE"))
    );
}

fn vad_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(super::vad::BUNDLED_VAD_RESOURCE_PATH)
}

struct NoopInference;

impl super::worker::LocalInferenceClient for NoopInference {
    fn transcribe(
        &self,
        _audio: &[f32],
        _language: Option<&str>,
        _abort_requested: Arc<AtomicBool>,
    ) -> Result<Option<String>, LocalTranscriptionError> {
        Ok(None)
    }
}

fn noop_ready_model(path: &Path) -> ReadyLocalModel {
    ReadyLocalModel::test_with_inference(path.to_path_buf(), Arc::new(NoopInference))
}

/// Decode the bundled diagnostic JFK sample to f32 and resample to 16 kHz
/// via the production converter. This fixture is repository-owned and does
/// not depend on the old git-ignored models/jfk.wav path.
fn jfk_audio() -> Vec<f32> {
    let wav_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/diagnostics_speech.wav");
    let mut reader = hound::WavReader::open(&wav_path).unwrap();
    let spec = reader.spec();
    assert_eq!(spec.channels, 1);
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect();
    crate::audio::convert::resample_to_16k_mono(&samples, spec.sample_rate).unwrap()
}

fn test_session_with_transcriber(transcriber: SegmentTranscriber) -> LocalSession {
    LocalSession::test_with_transcriber(
        &vad_model_path(),
        transcriber,
        LocalOutputCommitter::terminal_clipboard(),
    )
}

fn test_session_with_transcriber_and_output(
    transcriber: SegmentTranscriber,
    on_segment: LocalSegmentOutput,
) -> LocalSession {
    LocalSession::test_with_transcriber(
        &vad_model_path(),
        transcriber,
        LocalOutputCommitter::live(on_segment),
    )
}

fn terminal_text(output: LocalTerminalOutput) -> Option<String> {
    match output {
        LocalTerminalOutput::TerminalClipboard(text) => Some(text),
        LocalTerminalOutput::NoSpeech | LocalTerminalOutput::LiveCommitted => None,
    }
}

fn normalized_transcript_words(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch.is_whitespace() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(target_os = "windows")]
fn current_process_working_set_bytes() -> Option<usize> {
    use std::mem::{size_of, zeroed};
    use winapi::um::processthreadsapi::GetCurrentProcess;
    use winapi::um::psapi::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};

    unsafe {
        let mut counters: PROCESS_MEMORY_COUNTERS = zeroed();
        let ok = GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        );
        (ok != 0).then_some(counters.WorkingSetSize)
    }
}

#[cfg(not(target_os = "windows"))]
fn current_process_working_set_bytes() -> Option<usize> {
    None
}

fn fake_vulkan_device(
    whisper_gpu_ordinal: i32,
    kind: VulkanDeviceKind,
    memory_total: usize,
) -> VulkanDeviceInfo {
    VulkanDeviceInfo {
        whisper_gpu_ordinal,
        name: format!("device-{whisper_gpu_ordinal}"),
        description: format!("test device {whisper_gpu_ordinal}"),
        memory_free: memory_total / 2,
        memory_total,
        kind,
    }
}

#[test]
fn device_selection_prefers_discrete_over_integrated() {
    let devices = vec![
        fake_vulkan_device(0, VulkanDeviceKind::Integrated, 16),
        fake_vulkan_device(1, VulkanDeviceKind::Discrete, 8),
    ];
    assert_eq!(
        preferred_whisper_backend(&devices),
        PreferredWhisperBackend::Vulkan { gpu_ordinal: 1 }
    );
}

#[test]
fn device_selection_prefers_greatest_memory_among_discrete() {
    let devices = vec![
        fake_vulkan_device(0, VulkanDeviceKind::Discrete, 8),
        fake_vulkan_device(1, VulkanDeviceKind::Discrete, 24),
        fake_vulkan_device(2, VulkanDeviceKind::Discrete, 12),
    ];
    assert_eq!(
        preferred_whisper_backend(&devices),
        PreferredWhisperBackend::Vulkan { gpu_ordinal: 1 }
    );
}

#[test]
fn device_selection_uses_integrated_when_no_discrete_exists() {
    let devices = vec![
        fake_vulkan_device(0, VulkanDeviceKind::Integrated, 4),
        fake_vulkan_device(1, VulkanDeviceKind::Integrated, 8),
    ];
    assert_eq!(
        preferred_whisper_backend(&devices),
        PreferredWhisperBackend::Vulkan { gpu_ordinal: 1 }
    );
}

#[test]
fn device_selection_uses_cpu_when_no_gpu_exists() {
    assert_eq!(preferred_whisper_backend(&[]), PreferredWhisperBackend::Cpu);
}

#[test]
fn device_selection_uses_stable_enumeration_order_for_memory_ties() {
    let devices = vec![
        fake_vulkan_device(4, VulkanDeviceKind::Discrete, 16),
        fake_vulkan_device(9, VulkanDeviceKind::Discrete, 16),
    ];
    assert_eq!(
        preferred_whisper_backend(&devices),
        PreferredWhisperBackend::Vulkan { gpu_ordinal: 4 }
    );
}

#[test]
fn gpu_context_creation_failure_attempts_cpu_once() {
    let devices = vec![fake_vulkan_device(3, VulkanDeviceKind::Discrete, 16)];
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&attempts);
    let (context, selection) = create_context_with_fallback(
        Path::new("model.bin"),
        &devices,
        move |_, use_gpu, gpu_ordinal| {
            observed.lock().push((use_gpu, gpu_ordinal));
            if use_gpu {
                Err("gpu-init")
            } else {
                Ok("cpu-context")
            }
        },
    )
    .unwrap();
    assert_eq!(context, "cpu-context");
    assert_eq!(selection.backend, LocalInferenceBackend::Cpu);
    assert_eq!(selection.vulkan_device.as_deref(), Some("test device 3"));
    assert_eq!(
        selection.vulkan_device_kind,
        Some(VulkanDeviceKind::Discrete)
    );
    assert!(selection.cpu_fallback);
    assert_eq!(&*attempts.lock(), &[(true, 3), (false, 0)]);
}

#[test]
fn both_context_creation_attempts_fail_through_existing_model_load_route() {
    let devices = vec![fake_vulkan_device(2, VulkanDeviceKind::Discrete, 16)];
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&attempts);
    let result = create_context_with_fallback(
        Path::new("model.bin"),
        &devices,
        move |_, use_gpu, gpu_ordinal| -> Result<(), &'static str> {
            observed.lock().push((use_gpu, gpu_ordinal));
            Err(if use_gpu { "gpu-init" } else { "cpu-init" })
        },
    )
    .map_err(|_| LocalTranscriptionError::ModelLoad);
    assert_eq!(result, Err(LocalTranscriptionError::ModelLoad));
    assert_eq!(&*attempts.lock(), &[(true, 2), (false, 0)]);
}

#[test]
fn successful_gpu_context_does_not_create_or_retain_cpu_duplicate() {
    let devices = vec![fake_vulkan_device(5, VulkanDeviceKind::Discrete, 16)];
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&attempts);
    let (context, selection) = create_context_with_fallback(
        Path::new("model.bin"),
        &devices,
        move |_, use_gpu, gpu_ordinal| {
            observed.lock().push((use_gpu, gpu_ordinal));
            Ok::<_, ()>((use_gpu, gpu_ordinal))
        },
    )
    .unwrap();
    assert_eq!(context, (true, 5));
    assert_eq!(selection.backend, LocalInferenceBackend::Vulkan);
    assert_eq!(selection.vulkan_device.as_deref(), Some("test device 5"));
    assert_eq!(
        selection.vulkan_device_kind,
        Some(VulkanDeviceKind::Discrete)
    );
    assert!(!selection.cpu_fallback);
    assert_eq!(&*attempts.lock(), &[(true, 5)]);
}

#[test]
fn cpu_selection_report_has_no_vulkan_device_or_fallback() {
    let (context, selection) =
        create_context_with_fallback(Path::new("model.bin"), &[], |_, use_gpu, gpu_ordinal| {
            Ok::<_, ()>((use_gpu, gpu_ordinal))
        })
        .unwrap();

    assert_eq!(context, (false, 0));
    assert_eq!(selection.backend, LocalInferenceBackend::Cpu);
    assert_eq!(selection.vulkan_device, None);
    assert_eq!(selection.vulkan_device_kind, None);
    assert!(!selection.cpu_fallback);
}


#[test]
fn terminal_clipboard_committer_preserves_exact_ordered_text() {
    let mut committer = LocalOutputCommitter::terminal_clipboard();
    committer.commit(0, Some(" This".to_string())).unwrap();
    committer.commit(1, Some(" is test".to_string())).unwrap();
    committer.commit(2, None).unwrap();
    committer
        .commit(3, Some(" number two.".to_string()))
        .unwrap();
    committer
        .commit(4, Some(" [BLANK_AUDIO]".to_string()))
        .unwrap();
    assert_eq!(
        committer.finish(),
        LocalTerminalOutput::TerminalClipboard(
            " This is test number two. [BLANK_AUDIO]".to_string()
        )
    );

    let mut out_of_order = LocalOutputCommitter::terminal_clipboard();
    assert_eq!(
        out_of_order.commit(1, Some("wrong".to_string())),
        Err(LocalTranscriptionError::InferenceProtocol)
    );
}

#[test]
fn live_committer_delivers_once_without_retaining_transcript_text() {
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&delivered);
    let mut committer = LocalOutputCommitter::live(Arc::new(move |text| {
        observed.lock().push(text);
        Ok(())
    }));

    committer.commit(0, Some("first".to_string())).unwrap();
    committer.commit(1, None).unwrap();
    committer.commit(2, Some("second".to_string())).unwrap();
    assert_eq!(committer.retained_text_len(), 0);
    assert_eq!(committer.finish(), LocalTerminalOutput::LiveCommitted);
    assert_eq!(&*delivered.lock(), &["first", "second"]);
}

#[test]
fn live_committer_delivery_failure_is_terminal_and_retains_no_text() {
    let mut committer =
        LocalOutputCommitter::live(Arc::new(|_| Err(LocalTranscriptionError::Delivery)));
    assert_eq!(
        committer.commit(0, Some("cannot rollback".to_string())),
        Err(LocalTranscriptionError::Delivery)
    );
    assert_eq!(committer.retained_text_len(), 0);
}

#[test]
fn local_completion_is_terminal_and_emitted_only_once() {
    let completed = AtomicBool::new(false);
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&outcomes);
    let callback: LocalCompletion = Arc::new(move |result| observed.lock().push(result));

    complete_once(
        &completed,
        &callback,
        Err(LocalTranscriptionError::Inference),
    );
    complete_once(
        &completed,
        &callback,
        Ok(LocalTerminalOutput::LiveCommitted),
    );

    assert_eq!(
        &*outcomes.lock(),
        &[Err(LocalTranscriptionError::Inference)]
    );
}

#[test]
fn segment_inference_emits_exact_text_before_terminal_finish() {
    let expected = "verbatim segment".to_string();
    let transcribed = expected.clone();
    let (segment_tx, segment_rx) = mpsc::channel();
    let on_segment: LocalSegmentOutput = Arc::new(move |text| {
        segment_tx
            .send(text)
            .map_err(|_| LocalTranscriptionError::FeedClosed)
    });
    let mut worker = SegmentInferenceWorker::start_with_abort_and_output(
        Box::new(move |_, on_segment| {
            on_segment(transcribed.clone())?;
            Ok(Some(transcribed.clone()))
        }),
        Arc::new(AtomicBool::new(false)),
        on_segment,
    );

    worker.queue_segment(0, vec![0.1; 512]).unwrap();
    assert_eq!(
        segment_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        expected
    );
    assert_eq!(worker.finish().unwrap(), LocalTerminalOutput::LiveCommitted);
}

#[test]
fn segment_inference_worker_is_fifo_and_enqueue_does_not_wait_for_inference() {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let call_index = Arc::new(Mutex::new(0usize));
    let observed_index = Arc::clone(&call_index);

    let mut worker = SegmentInferenceWorker::start(Box::new(move |_, on_segment| {
        let mut index = observed_index.lock();
        let current = *index;
        *index += 1;
        drop(index);
        if current == 0 {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            on_segment("first".to_string())?;
            Ok(Some("first".to_string()))
        } else {
            on_segment("second".to_string())?;
            Ok(Some("second".to_string()))
        }
    }));

    worker.queue_segment(0, vec![0.1; 512]).unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let queued_at = std::time::Instant::now();
    worker.queue_segment(1, vec![0.2; 512]).unwrap();
    assert!(
        queued_at.elapsed() < Duration::from_millis(100),
        "enqueue must not wait for the active inference"
    );
    assert_eq!(worker.counts(), (2, 0));

    release_tx.send(()).unwrap();
    assert_eq!(
        worker.finish().unwrap(),
        LocalTerminalOutput::TerminalClipboard("firstsecond".to_string())
    );
    assert_eq!(worker.counts(), (2, 2));
}

#[test]
fn segment_inference_worker_rejects_duplicate_sequence_identity() {
    let mut worker = SegmentInferenceWorker::start(Box::new(|_, _| Ok(None)));
    worker.queue_segment(0, vec![0.0; 512]).unwrap();
    assert_eq!(
        worker.queue_segment(0, vec![0.0; 512]),
        Err(LocalTranscriptionError::InferenceProtocol)
    );
    let _ = worker.finish().unwrap();
}

#[test]
fn segment_inference_failure_preserves_decoded_clipboard_text() {
    let calls = Arc::new(Mutex::new(0usize));
    let observed = Arc::clone(&calls);
    let mut worker = SegmentInferenceWorker::start(Box::new(move |_, on_segment| {
        let mut calls = observed.lock();
        *calls += 1;
        if *calls == 2 {
            Err(LocalTranscriptionError::Inference)
        } else {
            on_segment("first segment".to_string())?;
            Ok(Some("first segment".to_string()))
        }
    }));

    worker.queue_segment(0, vec![0.1; 512]).unwrap();
    worker.queue_segment(1, vec![0.2; 512]).unwrap();
    worker.queue_segment(2, vec![0.3; 512]).unwrap();
    assert_eq!(
        worker.finish(),
        Ok(LocalTerminalOutput::TerminalClipboard("first segment".to_string()))
    );
    assert_eq!(
        *calls.lock(),
        2,
        "segments after terminal failure are not decoded"
    );
}

#[test]
fn stateful_local_silence_produces_no_transcript_or_segment_inference() {
    let inference_calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&inference_calls);
    let mut session = test_session_with_transcriber(Box::new(move |_, _| {
        observed.fetch_add(1, Ordering::SeqCst);
        Ok(Some("must never be produced".to_string()))
    }));

    for chunk in vec![0.0f32; crate::audio::convert::TARGET_SAMPLE_RATE as usize * 2].chunks(733) {
        session.feed_audio(chunk).unwrap();
    }

    assert_eq!(session.finalize().unwrap(), LocalTerminalOutput::NoSpeech);
    assert_eq!(
        inference_calls.load(Ordering::SeqCst),
        0,
        "silence must never enqueue a Whisper speech segment"
    );
}

#[test]
fn finalization_waits_for_queued_segment_inference_without_cancelling_it() {
    let (inference_started_tx, inference_started_rx) = mpsc::channel();
    let (release_inference_tx, release_inference_rx) = mpsc::channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);
    let mut session = test_session_with_transcriber(Box::new(move |_, on_segment| {
        let call = observed_calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            inference_started_tx.send(()).unwrap();
            release_inference_rx.recv().unwrap();
        }
        let text = format!("segment-{call}");
        on_segment(text.clone())?;
        Ok(Some(text))
    }));

    for chunk in jfk_audio().chunks(1600) {
        session.feed_audio(chunk).unwrap();
    }

    let (finalized_tx, finalized_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = finalized_tx.send(session.finalize());
    });

    inference_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("stateful VAD must queue at least one known-good speech segment");
    assert!(
            finalized_rx.recv_timeout(Duration::from_millis(75)).is_err(),
            "finalization must wait for queued Whisper inference rather than cancelling or bypassing it"
        );

    release_inference_tx.send(()).unwrap();
    let transcript = terminal_text(
        finalized_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("finalization must finish after queued inference is released")
            .expect("queued inference must succeed"),
    )
    .expect("known-good speech must assemble text");
    assert!(transcript.contains("segment-0"));
    assert!(calls.load(Ordering::SeqCst) >= 1);
}

#[test]
fn terminal_inference_failure_preserves_already_emitted_segments() {
    let (first_started_tx, first_started_rx) = mpsc::channel();
    let (release_first_tx, release_first_rx) = mpsc::channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let observed_emitted = Arc::clone(&emitted);
    let mut session = test_session_with_transcriber_and_output(
        Box::new(move |_, on_segment| {
            let call = observed_calls.fetch_add(1, Ordering::SeqCst);
            match call {
                0 => {
                    first_started_tx.send(()).unwrap();
                    release_first_rx.recv().unwrap();
                    on_segment("first segment".to_string())?;
                    Ok(Some("first segment".to_string()))
                }
                _ => Err(LocalTranscriptionError::Inference),
            }
        }),
        Arc::new(move |text| {
            observed_emitted.lock().push(text);
            Ok(())
        }),
    );

    let speech = jfk_audio();
    let mut audio = Vec::with_capacity(speech.len() * 2 + 16_000);
    audio.extend_from_slice(&speech);
    audio.extend(std::iter::repeat_n(0.0f32, 16_000));
    audio.extend_from_slice(&speech);
    for chunk in audio.chunks(1600) {
        session.feed_audio(chunk).unwrap();
    }

    let (finalized_tx, finalized_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = finalized_tx.send(session.finalize());
    });
    first_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first speech segment must reach inference");
    release_first_tx.send(()).unwrap();
    let result = finalized_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("terminal inference failure must finish the session");
    assert_eq!(result, Ok(LocalTerminalOutput::LiveCommitted));
    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "the test fixture must exercise a successful segment followed by failure"
    );
    assert_eq!(&*emitted.lock(), &["first segment"]);
}
#[test]
fn terminal_inference_failure_preserves_clipboard_text_in_session() {
    let (first_started_tx, first_started_rx) = mpsc::channel();
    let (release_first_tx, release_first_rx) = mpsc::channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);
    let mut session = test_session_with_transcriber(
        Box::new(move |_, on_segment| {
            let call = observed_calls.fetch_add(1, Ordering::SeqCst);
            match call {
                0 => {
                    first_started_tx.send(()).unwrap();
                    release_first_rx.recv().unwrap();
                    on_segment("first segment".to_string())?;
                    Ok(Some("first segment".to_string()))
                }
                _ => Err(LocalTranscriptionError::Inference),
            }
        }),
    );

    let speech = jfk_audio();
    let mut audio = Vec::with_capacity(speech.len() * 2 + 16_000);
    audio.extend_from_slice(&speech);
    audio.extend(std::iter::repeat_n(0.0f32, 16_000));
    audio.extend_from_slice(&speech);
    for chunk in audio.chunks(1600) {
        session.feed_audio(chunk).unwrap();
    }

    let (finalized_tx, finalized_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = finalized_tx.send(session.finalize());
    });
    first_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first speech segment must reach inference");
    release_first_tx.send(()).unwrap();
    let result = finalized_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("terminal inference failure must finish the session");
    assert_eq!(result, Ok(LocalTerminalOutput::TerminalClipboard("first segment".to_string())));
    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "the test fixture must exercise a successful segment followed by failure"
    );
}

#[test]
fn missing_model_preload_is_typed_before_worker_spawn() {
    let path = PathBuf::from("definitely/missing.bin");
    let authority = Arc::new(LocalRuntimeAuthority::new(LocalRuntimeTarget::Load(
        path.clone(),
    )));
    let intent = authority.current();
    let engine = LocalEngine::with_in_process_workers(authority);
    let (completed_tx, completed_rx) = mpsc::channel();

    engine.apply_intent(intent, move |path, result| {
        completed_tx.send((path, result)).unwrap();
    });

    assert_eq!(
        completed_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        (path, Err(LocalPreloadError::ModelNotFound))
    );
}

#[test]
fn invalid_model_worker_preload_is_typed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("garbage.bin");
    std::fs::write(&path, b"this is not a ggml model").unwrap();
    let authority = Arc::new(LocalRuntimeAuthority::new(LocalRuntimeTarget::Load(
        path.clone(),
    )));
    let intent = authority.current();
    let engine = LocalEngine::with_in_process_workers(authority);
    let (completed_tx, completed_rx) = mpsc::channel();

    engine.apply_intent(intent, move |path, result| {
        completed_tx.send((path, result)).unwrap();
    });

    assert_eq!(
        completed_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        (path, Err(LocalPreloadError::UnsupportedModel))
    );
}

#[test]
fn local_session_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<LocalSession>();
}

#[test]
fn session_rejects_invalid_input_sample_rate_before_model_or_vad_loading() {
    let engine = LocalEngine::new();
    assert!(matches!(
        engine.start_ready_session(
            Path::new("not-needed.bin"),
            Some("en"),
            0,
            Path::new("not-needed-vad.bin"),
        ),
        Err(LocalTranscriptionError::AudioConversion(
            AudioConversionError::InvalidSampleRate
        ))
    ));
}

#[test]
fn live_audio_feed_rejects_invalid_rate_through_existing_structured_error() {
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&outcomes);
    let feed = build_test_audio_feed(
        noop_ready_model(Path::new("model-is-not-reached.bin")),
        Some("en".to_string()),
        vad_model_path(),
        |_| Ok(()),
        move |outcome| observed.lock().push(outcome),
    );
    assert!(!feed.start(0));
    assert_eq!(
        &*outcomes.lock(),
        &[Err(LocalTranscriptionError::AudioConversion(
            AudioConversionError::InvalidSampleRate,
        ))]
    );
}
#[test]
fn compiled_segment_delivery_and_max_length_match_build_env() {
    let expected_delivery = match env!("PORTUS_LOCAL_SEGMENT_DELIVERY") {
        "true" => true,
        _ => false,
    };
    assert_eq!(super::COMPILED_SEGMENT_DELIVERY, expected_delivery);

    let expected_max_len: i32 = env!("PORTUS_LOCAL_MAX_LENGTH").parse().unwrap_or(0);
    assert_eq!(super::COMPILED_MAX_LENGTH, expected_max_len);
}

#[test]
fn output_committer_handles_streaming_chunks_for_live_and_clipboard() {
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let delivered_clone = Arc::clone(&delivered);
    let mut live_committer = LocalOutputCommitter::live(Arc::new(move |chunk| {
        delivered_clone.lock().push(chunk);
        Ok(())
    }));

    live_committer.commit_chunk(0, "first chunk".to_string()).unwrap();
    live_committer.commit_chunk(0, " second chunk".to_string()).unwrap();
    live_committer.advance_sequence(0).unwrap();

    assert_eq!(
        &*delivered.lock(),
        &["first chunk".to_string(), " second chunk".to_string()]
    );
    assert_eq!(live_committer.finish(), LocalTerminalOutput::LiveCommitted);

    let mut clip_committer = LocalOutputCommitter::terminal_clipboard();
    clip_committer.commit_chunk(0, "first".to_string()).unwrap();
    clip_committer.commit_chunk(0, " second".to_string()).unwrap();
    clip_committer.advance_sequence(0).unwrap();

    assert_eq!(
        clip_committer.finish(),
        LocalTerminalOutput::TerminalClipboard("first second".to_string())
    );
}

#[test]
fn output_committer_rejects_out_of_order_sequence() {
    let mut committer = LocalOutputCommitter::terminal_clipboard();
    assert_eq!(
        committer.commit_chunk(1, "invalid sequence".to_string()),
        Err(LocalTranscriptionError::InferenceProtocol)
    );
    assert_eq!(
        committer.advance_sequence(1),
        Err(LocalTranscriptionError::InferenceProtocol)
    );
}
