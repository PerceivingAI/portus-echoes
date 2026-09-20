use super::*;
use crate::audio::device::capture_input_block;
use std::sync::atomic::{AtomicBool, AtomicUsize};

struct TestStream {
    starts: bool,
}

struct FailureOnDropStream {
    failure_state: CaptureFailureState,
    kind: CaptureFailureKind,
}

impl CaptureStream for FailureOnDropStream {
    fn play(&self) -> bool {
        true
    }
}

impl Drop for FailureOnDropStream {
    fn drop(&mut self) {
        self.failure_state.record(self.kind);
    }
}

impl CaptureStream for TestStream {
    fn play(&self) -> bool {
        self.starts
    }
}

fn test_session(
    generation: u64,
    starts: bool,
    destination: CaptureDestination,
    samples: Vec<f32>,
) -> Session {
    if let CaptureDestination::CloudCompleted(buffer) = &destination {
        buffer.samples.lock().extend(samples);
    } else {
        assert!(
            samples.is_empty(),
            "non-completed test sessions cannot own completed Cloud PCM"
        );
    }
    Session {
        recording_id: RecordingIdentity(1),
        stream: Box::new(TestStream { starts }),
        destination,
        sample_rate: TARGET_SAMPLE_RATE,
        generation,
        failure_state: CaptureFailureState::default(),
    }
}

fn test_session_for(
    recording_id: RecordingIdentity,
    generation: u64,
    starts: bool,
    destination: CaptureDestination,
    samples: Vec<f32>,
) -> Session {
    let mut session = test_session(generation, starts, destination, samples);
    session.recording_id = recording_id;
    session
}

fn wait_until(predicate: impl Fn() -> bool) {
    for _ in 0..100 {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(predicate(), "condition was not reached");
}

fn event_feed(events: Arc<Mutex<Vec<String>>>, stop_ok: bool) -> LocalAudioFeed {
    let started = Arc::clone(&events);
    let samples = Arc::clone(&events);
    let stopped = Arc::clone(&events);
    let aborted = Arc::clone(&events);
    LocalAudioFeed::new(
        move |rate| {
            started.lock().push(format!("start:{rate}"));
            true
        },
        move |pcm| {
            samples.lock().push(format!("samples:{pcm:?}"));
            Ok(())
        },
        move || {
            stopped.lock().push("stop".to_string());
            stop_ok
        },
        move || aborted.lock().push("abort".to_string()),
    )
}

#[test]
fn local_audio_feed_receives_downmixed_pcm_before_stop() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let feed = event_feed(Arc::clone(&events), true);

    assert!(feed.start(48_000));
    let destination = CaptureDestination::LocalLive(feed.clone());
    assert_eq!(
        capture_input_block(&[1.0f32, -1.0, 0.5, 0.5], 2, |sample| sample, &destination),
        Ok(())
    );
    assert_eq!(
        &*events.lock(),
        &["start:48000".to_string(), "samples:[0.0, 0.5]".to_string()]
    );
    assert!(feed.stop());
    assert_eq!(events.lock().last().map(String::as_str), Some("stop"));
}

#[test]
fn cloud_capture_only_updates_complete_buffer() {
    let buffer = CloudCaptureBuffer::default();
    let destination = CaptureDestination::CloudCompleted(buffer.clone());
    assert_eq!(
        capture_input_block(&[0.25f32, -0.5, 0.75], 1, |sample| sample, &destination),
        Ok(())
    );
    assert_eq!(&*buffer.samples.lock(), &[0.25, -0.5, 0.75]);
}

#[test]
fn cloud_live_feed_receives_downmixed_pcm_and_stops_without_completed_payload() {
    let feed_events = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::clone(&feed_events);
    let pushed = Arc::clone(&feed_events);
    let stopped = Arc::clone(&feed_events);
    let aborted = Arc::clone(&feed_events);
    let feed = CloudLiveFeed::new(
        move |sample_rate| {
            started.lock().push(format!("start:{sample_rate}"));
            Ok(())
        },
        move |samples| {
            pushed.lock().push(format!("samples:{samples:?}"));
            Ok(())
        },
        move || {
            stopped.lock().push("stop".to_string());
            Ok(())
        },
        move || aborted.lock().push("abort".to_string()),
    );
    let capture_events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&capture_events);
    let (engine, rx) = AudioEngine::new(move |event| observed.lock().push(event));
    let id = RecordingIdentity(77);

    assert!(engine.start_cloud_live_with_identity(
        id,
        feed,
        Some(Duration::from_secs(30)),
        Arc::new(|| true),
        |_, generation, recording_id, destination| {
            Some(test_session_for(
                recording_id,
                generation,
                true,
                destination,
                Vec::new(),
            ))
        },
    ));
    let destination = engine.session.lock().as_ref().unwrap().destination.clone();
    assert_eq!(
        capture_input_block(&[1.0f32, -1.0, 0.5, 0.5], 2, |sample| sample, &destination),
        Ok(())
    );
    assert!(engine.stop_for(id));

    assert_eq!(
        &*feed_events.lock(),
        &["start:16000", "samples:[0.0, 0.5]", "stop"]
    );
    assert_eq!(
        &*capture_events.lock(),
        &[
            CaptureEvent {
                recording_id: id,
                kind: CaptureEventKind::Started,
            },
            CaptureEvent {
                recording_id: id,
                kind: CaptureEventKind::Stopped,
            },
        ]
    );
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn capture_start_apis_construct_only_the_requested_destination() {
    let (engine, rx) = AudioEngine::new(|_| {});

    let completed_id = RecordingIdentity(90);
    assert!(engine.start_cloud_completed_with_identity(
        completed_id,
        Duration::from_secs(30),
        Arc::new(|| true),
        |_, generation, recording_id, destination| {
            assert!(matches!(destination, CaptureDestination::CloudCompleted(_)));
            Some(test_session_for(
                recording_id,
                generation,
                true,
                destination,
                Vec::new(),
            ))
        },
    ));
    assert!(engine.abort_for(completed_id));

    let live_id = RecordingIdentity(91);
    let live_feed = CloudLiveFeed::new(|_| Ok(()), |_| Ok(()), || Ok(()), || {});
    assert!(engine.start_cloud_live_with_identity(
        live_id,
        live_feed,
        Some(Duration::from_secs(30)),
        Arc::new(|| true),
        |_, generation, recording_id, destination| {
            assert!(matches!(destination, CaptureDestination::CloudLive(_)));
            Some(test_session_for(
                recording_id,
                generation,
                true,
                destination,
                Vec::new(),
            ))
        },
    ));
    assert!(engine.abort_for(live_id));

    let local_id = RecordingIdentity(92);
    let local_feed = LocalAudioFeed::new(|_| true, |_| Ok(()), || true, || {});
    assert!(engine.start_local_with_identity(
        local_id,
        local_feed,
        Arc::new(|| true),
        |_, generation, recording_id, destination| {
            assert!(matches!(destination, CaptureDestination::LocalLive(_)));
            Some(test_session_for(
                recording_id,
                generation,
                true,
                destination,
                Vec::new(),
            ))
        },
    ));
    assert!(engine.abort_for(local_id));

    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn revoked_live_start_guard_prevents_feed_and_microphone_start_after_preparation() {
    let feed_events = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::clone(&feed_events);
    let aborted = Arc::clone(&feed_events);
    let feed = CloudLiveFeed::new(
        move |_| {
            started.lock().push("start".to_string());
            Ok(())
        },
        |_| Ok(()),
        || Ok(()),
        move || aborted.lock().push("abort".to_string()),
    );
    let capture_events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&capture_events);
    let (engine, rx) = AudioEngine::new(move |event| observed.lock().push(event));
    let allowed = Arc::new(AtomicBool::new(true));
    let start_allowed = {
        let allowed = Arc::clone(&allowed);
        Arc::new(move || allowed.load(Ordering::SeqCst)) as StartGuard
    };
    let id = RecordingIdentity(93);

    assert!(!engine.start_cloud_live_with_identity(
        id,
        feed,
        Some(Duration::from_secs(30)),
        start_allowed,
        {
            let allowed = Arc::clone(&allowed);
            move |_, generation, recording_id, destination| {
                allowed.store(false, Ordering::SeqCst);
                Some(test_session_for(
                    recording_id,
                    generation,
                    true,
                    destination,
                    Vec::new(),
                ))
            }
        },
    ));

    assert!(!engine.is_active());
    assert_eq!(&*feed_events.lock(), &["abort"]);
    assert!(capture_events.lock().is_empty());
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn cloud_live_feed_start_failure_is_typed_and_aborted() {
    let feed_events = Arc::new(Mutex::new(Vec::new()));
    let aborted = Arc::clone(&feed_events);
    let feed = CloudLiveFeed::new(
        |_| Err(CloudLiveFeedError::Closed),
        |_| Ok(()),
        || Ok(()),
        move || aborted.lock().push("abort".to_string()),
    );
    let capture_events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&capture_events);
    let (engine, rx) = AudioEngine::new(move |event| observed.lock().push(event));
    let id = RecordingIdentity(78);

    assert!(!engine.start_cloud_live_with_identity(
        id,
        feed,
        Some(Duration::from_secs(30)),
        Arc::new(|| true),
        |_, generation, recording_id, destination| {
            Some(test_session_for(
                recording_id,
                generation,
                true,
                destination,
                Vec::new(),
            ))
        },
    ));
    assert!(!engine.is_active());
    assert_eq!(&*feed_events.lock(), &["abort"]);
    assert_eq!(
        &*capture_events.lock(),
        &[CaptureEvent {
            recording_id: id,
            kind: CaptureEventKind::Failed(CaptureFailureKind::FeedClosed),
        }]
    );
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn cloud_live_backpressure_is_distinct_and_never_buffers_completed_audio() {
    let feed = CloudLiveFeed::new(
        |_| Ok(()),
        |_| Err(CloudLiveFeedError::Backpressure),
        || Ok(()),
        || {},
    );
    assert_eq!(
        capture_input_block(
            &[0.25f32, -0.5, 0.75],
            1,
            |sample| sample,
            &CaptureDestination::CloudLive(feed),
        ),
        Err(CaptureFailureKind::CloudBackpressure)
    );
}

#[test]
fn cloud_live_stop_failure_reports_failure_and_aborts_without_stopped_event() {
    let feed_events = Arc::new(Mutex::new(Vec::new()));
    let aborted = Arc::clone(&feed_events);
    let feed = CloudLiveFeed::new(
        |_| Ok(()),
        |_| Ok(()),
        || Err(CloudLiveFeedError::Closed),
        move || aborted.lock().push("abort".to_string()),
    );
    let capture_events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&capture_events);
    let (engine, rx) = AudioEngine::new(move |event| observed.lock().push(event));
    let id = RecordingIdentity(79);

    assert!(engine.start_cloud_live_with_identity(
        id,
        feed,
        Some(Duration::from_secs(30)),
        Arc::new(|| true),
        |_, generation, recording_id, destination| {
            Some(test_session_for(
                recording_id,
                generation,
                true,
                destination,
                Vec::new(),
            ))
        },
    ));
    assert!(engine.stop_for(id));
    assert_eq!(&*feed_events.lock(), &["abort"]);
    assert_eq!(
        &*capture_events.lock(),
        &[
            CaptureEvent {
                recording_id: id,
                kind: CaptureEventKind::Started,
            },
            CaptureEvent {
                recording_id: id,
                kind: CaptureEventKind::Failed(CaptureFailureKind::FeedClosed),
            },
        ]
    );
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn concurrent_live_stop_reports_already_stopping_instead_of_missing_capture() {
    let (stop_entered_tx, stop_entered_rx) = std::sync::mpsc::channel();
    let (release_stop_tx, release_stop_rx) = std::sync::mpsc::channel();
    let release_stop_rx = Arc::new(Mutex::new(release_stop_rx));
    let feed = CloudLiveFeed::new(
        |_| Ok(()),
        |_| Ok(()),
        {
            let release_stop_rx = Arc::clone(&release_stop_rx);
            move || {
                stop_entered_tx.send(()).unwrap();
                release_stop_rx.lock().recv().unwrap();
                Ok(())
            }
        },
        || {},
    );
    let (engine, _rx) = AudioEngine::new(|_| {});
    let id = RecordingIdentity(801);
    assert!(engine.start_cloud_live_with_identity(
        id,
        feed,
        Some(Duration::from_secs(30)),
        Arc::new(|| true),
        |_, generation, recording_id, destination| {
            Some(test_session_for(
                recording_id,
                generation,
                true,
                destination,
                Vec::new(),
            ))
        },
    ));

    let stopping_engine = Arc::clone(&engine);
    let stop_thread = std::thread::spawn(move || stopping_engine.stop_result_for(id));
    stop_entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first exact stop must enter live feed finalization");

    assert_eq!(
        engine.stop_result_for(id),
        StopCaptureResult::AlreadyStopping,
        "a second exact terminal interaction must not be mistaken for a preparation-time missing capture"
    );

    release_stop_tx.send(()).unwrap();
    assert_eq!(stop_thread.join().unwrap(), StopCaptureResult::Stopped);
    assert_eq!(engine.stop_result_for(id), StopCaptureResult::NotFound);
}

#[test]
fn cloud_live_recording_limit_matches_manual_stop_state_contract() {
    fn run(manual: bool) -> (Vec<String>, Vec<CaptureEvent>) {
        let feed_events = Arc::new(Mutex::new(Vec::new()));
        let started = Arc::clone(&feed_events);
        let stopped = Arc::clone(&feed_events);
        let aborted = Arc::clone(&feed_events);
        let feed = CloudLiveFeed::new(
            move |sample_rate| {
                started.lock().push(format!("start:{sample_rate}"));
                Ok(())
            },
            |_| Ok(()),
            move || {
                stopped.lock().push("stop".to_string());
                Ok(())
            },
            move || aborted.lock().push("abort".to_string()),
        );
        let capture_events = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&capture_events);
        let (engine, rx) = AudioEngine::new(move |event| observed.lock().push(event));
        let id = RecordingIdentity(80);

        assert!(engine.start_cloud_live_with_identity(
            id,
            feed,
            Some(Duration::from_millis(30)),
            Arc::new(|| true),
            |_, generation, recording_id, destination| {
                Some(test_session_for(
                    recording_id,
                    generation,
                    true,
                    destination,
                    Vec::new(),
                ))
            },
        ));
        if manual {
            assert!(engine.stop_for(id));
        } else {
            wait_until(|| !engine.is_active());
        }

        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        let feed_history = feed_events.lock().clone();
        let capture_history = capture_events.lock().clone();
        (feed_history, capture_history)
    }

    let manual = run(true);
    let automatic = run(false);
    assert_eq!(manual, automatic);
    assert_eq!(manual.0, vec!["start:16000", "stop"]);
    assert_eq!(
        manual.1,
        vec![
            CaptureEvent {
                recording_id: RecordingIdentity(80),
                kind: CaptureEventKind::Started,
            },
            CaptureEvent {
                recording_id: RecordingIdentity(80),
                kind: CaptureEventKind::Stopped,
            },
        ]
    );
}

#[test]
fn failed_live_handoff_is_not_silent_without_a_complete_buffer() {
    let feed = LocalAudioFeed::new(
        |_| true,
        |_| Err(LocalAudioFeedError::FeedClosed),
        || true,
        || {},
    );
    let destination = CaptureDestination::LocalLive(feed);
    assert_eq!(
        capture_input_block(&[0.25f32, 0.5], 1, |sample| sample, &destination),
        Err(CaptureFailureKind::FeedClosed)
    );
}

#[test]
fn cloud_live_feed_error_mapping_preserves_closed_and_backpressure_identity() {
    assert_eq!(
        CaptureFailureKind::from(CloudLiveFeedError::Closed),
        CaptureFailureKind::FeedClosed
    );
    assert_eq!(
        CaptureFailureKind::from(CloudLiveFeedError::Backpressure),
        CaptureFailureKind::CloudBackpressure
    );
}

#[test]
fn local_handoff_failure_kind_preserves_backpressure_identity() {
    assert_eq!(
        CaptureFailureKind::from(LocalAudioFeedError::FeedClosed),
        CaptureFailureKind::FeedClosed
    );
    assert_eq!(
        CaptureFailureKind::from(LocalAudioFeedError::Backpressure),
        CaptureFailureKind::LocalBackpressure
    );
}

#[test]
fn local_audio_feed_start_failure_prevents_recording_start() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let feed = LocalAudioFeed::new(|_| false, |_| Ok(()), || true, {
        let events = Arc::clone(&events);
        move || events.lock().push("abort".to_string())
    });
    let (engine, rx) = AudioEngine::new(|_| {});
    assert!(
        !engine.start_local_with(feed, |_, generation, destination| {
            Some(test_session(generation, true, destination, Vec::new()))
        })
    );
    assert!(!engine.is_active());
    assert_eq!(&*events.lock(), &["abort".to_string()]);
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn failed_live_stop_never_falls_back_to_complete_buffer_delivery() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let feed = event_feed(events, false);
    let (engine, rx) = AudioEngine::new(|_| {});
    assert!(engine.start_local_with(feed, |_, generation, destination| {
        Some(test_session(generation, true, destination, Vec::new()))
    }));
    engine.stop();
    assert!(!engine.is_active());
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn successful_live_stop_is_consumed_without_complete_buffer_delivery() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let feed = event_feed(Arc::clone(&events), true);
    let (engine, rx) = AudioEngine::new(|_| {});
    assert!(engine.start_local_with(feed, |_, generation, destination| {
        Some(test_session(generation, true, destination, Vec::new()))
    }));

    engine.stop();

    assert!(!engine.is_active());
    assert_eq!(
        events
            .lock()
            .iter()
            .filter(|event| event.as_str() == "stop")
            .count(),
        1
    );
    assert!(
            rx.recv_timeout(Duration::from_millis(30)).is_err(),
            "a successful live feed owns the recording outcome and must not also emit CloudCapturedAudio"
        );
}

#[test]
fn live_recording_start_and_stop_are_idempotent_and_finalize_once() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let feed = event_feed(Arc::clone(&events), true);
    let (engine, rx) = AudioEngine::new(|_| {});
    let open_calls = Arc::new(AtomicUsize::new(0));
    let first_calls = Arc::clone(&open_calls);

    assert!(
        engine.start_local_with(feed.clone(), move |_, generation, destination| {
            first_calls.fetch_add(1, Ordering::SeqCst);
            Some(test_session(generation, true, destination, Vec::new()))
        },)
    );

    let duplicate_calls = Arc::clone(&open_calls);
    assert!(engine.start_local_with(feed, move |_, _, _| {
        duplicate_calls.fetch_add(1, Ordering::SeqCst);
        panic!("duplicate start must not construct a second session")
    }));

    engine.stop();
    engine.stop();

    assert_eq!(open_calls.load(Ordering::SeqCst), 1);
    let events = events.lock();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.starts_with("start:"))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.as_str() == "stop")
            .count(),
        1
    );
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn live_worker_may_finish_after_capture_state_is_already_inactive() {
    let finished = Arc::new(AtomicBool::new(false));
    let stop_finished = Arc::clone(&finished);
    let feed = LocalAudioFeed::new(
        |_| true,
        |_| Ok(()),
        move || {
            let finished = Arc::clone(&stop_finished);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(75));
                finished.store(true, Ordering::SeqCst);
            });
            true
        },
        || {},
    );
    let (engine, rx) = AudioEngine::new(|_| {});

    assert!(engine.start_local_with(feed, |_, generation, destination| {
        Some(test_session(generation, true, destination, Vec::new()))
    }));
    engine.stop();

    assert!(!engine.is_active());
    assert!(!finished.load(Ordering::SeqCst));
    wait_until(|| finished.load(Ordering::SeqCst));
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn local_capture_has_no_cloud_timer_deadline() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let feed = event_feed(Arc::clone(&events), true);
    let (engine, rx) = AudioEngine::new(|_| {});
    assert!(engine.start_local_with(feed, |_, generation, destination| {
        Some(test_session(generation, true, destination, Vec::new()))
    }));

    std::thread::sleep(Duration::from_millis(60));
    assert!(
        engine.is_active(),
        "Local capture must not inherit the Cloud recording limit"
    );
    assert_eq!(
        events
            .lock()
            .iter()
            .filter(|event| event.as_str() == "stop")
            .count(),
        0
    );
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());

    engine.stop();
    assert!(!engine.is_active());
    assert_eq!(
        events
            .lock()
            .iter()
            .filter(|event| event.as_str() == "stop")
            .count(),
        1
    );
}

#[test]
fn capture_open_failure_returns_false_without_state_or_audio() {
    let states = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&states);
    let (engine, rx) = AudioEngine::new(move |state| observed.lock().push(state));

    assert!(!engine.start_cloud_completed_with(Duration::from_secs(30), |_, _, _| None));
    assert!(!engine.is_active());
    assert!(states.lock().is_empty());
    assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
}

#[test]
fn stream_start_failure_returns_false_without_state_or_audio() {
    let states = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&states);
    let (engine, rx) = AudioEngine::new(move |state| observed.lock().push(state));

    assert!(!engine.start_cloud_completed_with(
        Duration::from_secs(30),
        |_, generation, destination| {
            Some(test_session(generation, false, destination, vec![0.5; 32]))
        }
    ));
    assert!(!engine.is_active());
    assert!(states.lock().is_empty());
    assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
}

#[test]
fn live_stream_failure_aborts_and_discards_audio() {
    let states = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&states);
    let (engine, rx) = AudioEngine::new(move |state| observed.lock().push(state));

    assert!(engine.start_cloud_completed_with(
        Duration::from_secs(30),
        |_, generation, destination| {
            Some(test_session(generation, true, destination, vec![0.5; 32]))
        }
    ));
    let generation = engine.session.lock().as_ref().unwrap().generation;
    engine
        .capture_failure_tx
        .send(CaptureFailure {
            generation,
            kind: CaptureFailureKind::Stream,
        })
        .unwrap();

    wait_until(|| {
        states
            .lock()
            .iter()
            .any(|event| event.kind == CaptureEventKind::Failed(CaptureFailureKind::Stream))
    });
    assert!(!engine.is_active());
    assert_eq!(
        &*states.lock(),
        &[
            CaptureEvent::Started,
            CaptureEvent::Failed(CaptureFailureKind::Stream),
        ]
    );
    assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
}

#[test]
fn latched_local_backpressure_preempts_successful_stop() {
    let feed_events = Arc::new(Mutex::new(Vec::new()));
    let feed = event_feed(Arc::clone(&feed_events), true);
    let capture_events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&capture_events);
    let (engine, rx) = AudioEngine::new(move |event| observed.lock().push(event));

    assert!(engine.start_local_with(feed, |_, generation, destination| {
        Some(test_session(generation, true, destination, Vec::new()))
    }));
    engine
        .session
        .lock()
        .as_ref()
        .unwrap()
        .failure_state
        .record(CaptureFailureKind::LocalBackpressure);

    assert!(engine.stop_for(RecordingIdentity(1)));
    assert!(!engine.is_active());
    assert_eq!(
        &*capture_events.lock(),
        &[
            CaptureEvent::Started,
            CaptureEvent::Failed(CaptureFailureKind::LocalBackpressure),
        ]
    );
    let feed_events = feed_events.lock();
    assert!(feed_events.iter().any(|event| event == "abort"));
    assert!(!feed_events.iter().any(|event| event == "stop"));
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn failure_latched_while_stream_is_dropped_preempts_successful_stop() {
    let capture_events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&capture_events);
    let (engine, rx) = AudioEngine::new(move |event| observed.lock().push(event));
    let buffer = CloudCaptureBuffer::default();
    buffer.samples.lock().extend([0.5; 16]);
    let failure_state = CaptureFailureState::default();

    *engine.session.lock() = Some(Session {
        recording_id: RecordingIdentity(1),
        stream: Box::new(FailureOnDropStream {
            failure_state: failure_state.clone(),
            kind: CaptureFailureKind::Stream,
        }),
        destination: CaptureDestination::CloudCompleted(buffer),
        sample_rate: TARGET_SAMPLE_RATE,
        generation: 1,
        failure_state,
    });

    assert!(engine.stop_for(RecordingIdentity(1)));
    assert_eq!(
        &*capture_events.lock(),
        &[CaptureEvent::Failed(CaptureFailureKind::Stream)]
    );
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn latched_cloud_stream_failure_preempts_completed_buffer_stop() {
    let capture_events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&capture_events);
    let (engine, rx) = AudioEngine::new(move |event| observed.lock().push(event));

    assert!(engine.start_cloud_completed_with(
        Duration::from_secs(30),
        |_, generation, destination| {
            Some(test_session(generation, true, destination, vec![0.5; 32]))
        }
    ));
    engine
        .session
        .lock()
        .as_ref()
        .unwrap()
        .failure_state
        .record(CaptureFailureKind::Stream);

    assert!(engine.stop_for(RecordingIdentity(1)));
    assert!(!engine.is_active());
    assert_eq!(
        &*capture_events.lock(),
        &[
            CaptureEvent::Started,
            CaptureEvent::Failed(CaptureFailureKind::Stream),
        ]
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(30)).is_err(),
        "a capture with an already-observed stream failure must not emit completed Cloud audio"
    );
}

#[test]
fn stale_stream_failure_does_not_abort_new_session() {
    let (engine, rx) = AudioEngine::new(|_| {});

    assert!(engine.start_cloud_completed_with(
        Duration::from_secs(30),
        |_, generation, destination| {
            Some(test_session(generation, true, destination, vec![0.25; 8]))
        }
    ));
    let stale_generation = engine.session.lock().as_ref().unwrap().generation;
    engine.stop();
    let _ = rx.recv_timeout(Duration::from_millis(50)).unwrap();

    assert!(engine.start_cloud_completed_with(
        Duration::from_secs(30),
        |_, generation, destination| {
            Some(test_session(generation, true, destination, vec![0.75; 8]))
        }
    ));
    engine
        .capture_failure_tx
        .send(CaptureFailure {
            generation: stale_generation,
            kind: CaptureFailureKind::Stream,
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(25));
    assert!(engine.is_active());
    engine.stop();
    let captured = rx.recv_timeout(Duration::from_millis(50)).unwrap();
    assert_eq!(captured.samples, vec![0.75; 8]);
}

#[test]
fn capture_events_retain_the_originating_recording_identity() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&events);
    let (engine, rx) = AudioEngine::new(move |event| observed.lock().push(event));
    let id = RecordingIdentity(41);

    assert!(engine.start_cloud_completed_with_identity(
        id,
        Duration::from_secs(30),
        Arc::new(|| true),
        |_, generation, recording_id, destination| {
            Some(test_session_for(
                recording_id,
                generation,
                true,
                destination,
                vec![0.5; 8],
            ))
        },
    ));
    assert!(engine.stop_for(id));
    let captured = rx.recv_timeout(Duration::from_millis(50)).unwrap();
    assert_eq!(captured.recording_id, id);

    assert_eq!(
        &*events.lock(),
        &[
            CaptureEvent {
                recording_id: id,
                kind: CaptureEventKind::Started,
            },
            CaptureEvent {
                recording_id: id,
                kind: CaptureEventKind::Stopped,
            },
        ]
    );
}

#[test]
fn aborted_cloud_session_emits_no_completed_buffer() {
    let (engine, rx) = AudioEngine::new(|_| {});
    let id = RecordingIdentity(42);

    assert!(engine.start_cloud_completed_with_identity(
        id,
        Duration::from_secs(30),
        Arc::new(|| true),
        |_, generation, recording_id, destination| {
            Some(test_session_for(
                recording_id,
                generation,
                true,
                destination,
                vec![0.5; 8],
            ))
        },
    ));
    assert!(engine.abort_for(id));
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn stale_stop_cannot_stop_a_newer_recording_identity() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&events);
    let (engine, rx) = AudioEngine::new(move |event| observed.lock().push(event));
    let old_id = RecordingIdentity(51);
    let new_id = RecordingIdentity(52);

    assert!(engine.start_cloud_completed_with_identity(
        old_id,
        Duration::from_secs(30),
        Arc::new(|| true),
        |_, generation, recording_id, destination| {
            Some(test_session_for(
                recording_id,
                generation,
                true,
                destination,
                Vec::new(),
            ))
        },
    ));
    assert!(engine.start_cloud_completed_with_identity(
        new_id,
        Duration::from_secs(30),
        Arc::new(|| true),
        |_, generation, recording_id, destination| {
            Some(test_session_for(
                recording_id,
                generation,
                true,
                destination,
                Vec::new(),
            ))
        },
    ));

    assert!(
        rx.recv_timeout(Duration::from_millis(30)).is_err(),
        "superseding a Cloud capture must not emit the old recording buffer"
    );
    assert!(!engine.stop_for(old_id));
    assert!(engine.is_active());
    assert_eq!(
        engine
            .session
            .lock()
            .as_ref()
            .map(|session| session.recording_id),
        Some(new_id)
    );
    assert!(engine.stop_for(new_id));
    let captured = rx.recv_timeout(Duration::from_millis(50)).unwrap();
    assert_eq!(captured.recording_id, new_id);
    assert!(events
        .lock()
        .iter()
        .any(|event| { event.recording_id == old_id && event.kind == CaptureEventKind::Aborted }));
}

#[test]
fn revoked_start_guard_prevents_capture_after_preparation() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&events);
    let (engine, rx) = AudioEngine::new(move |event| observed.lock().push(event));
    let allowed = Arc::new(AtomicBool::new(true));
    let start_allowed = {
        let allowed = Arc::clone(&allowed);
        Arc::new(move || allowed.load(Ordering::SeqCst)) as StartGuard
    };
    let id = RecordingIdentity(61);

    assert!(!engine.start_cloud_completed_with_identity(
        id,
        Duration::from_secs(30),
        start_allowed,
        {
            let allowed = Arc::clone(&allowed);
            move |_, generation, recording_id, destination| {
                allowed.store(false, Ordering::SeqCst);
                Some(test_session_for(
                    recording_id,
                    generation,
                    true,
                    destination,
                    Vec::new(),
                ))
            }
        },
    ));

    assert!(!engine.is_active());
    assert!(events.lock().is_empty());
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn capture_is_delivered_only_after_stop_and_exactly_once() {
    let states = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&states);
    let (engine, rx) = AudioEngine::new(move |state| observed.lock().push(state));
    let expected = vec![0.25, -0.5, 0.75];

    assert!(engine.start_cloud_completed_with(
        Duration::from_secs(30),
        |_, generation, destination| {
            Some(test_session(
                generation,
                true,
                destination,
                expected.clone(),
            ))
        }
    ));
    assert!(engine.is_active());
    assert_eq!(&*states.lock(), &[CaptureEvent::Started]);
    assert!(
        rx.recv_timeout(Duration::from_millis(30)).is_err(),
        "captured audio must not reach transcription before stop/release"
    );

    engine.stop();
    let captured = rx.recv_timeout(Duration::from_millis(50)).unwrap();
    assert_eq!(captured.samples, expected);
    assert_eq!(
        &*states.lock(),
        &[CaptureEvent::Started, CaptureEvent::Stopped]
    );

    engine.stop();
    assert!(
        rx.recv_timeout(Duration::from_millis(30)).is_err(),
        "one recording must emit exactly one captured buffer"
    );
    assert_eq!(
        &*states.lock(),
        &[CaptureEvent::Started, CaptureEvent::Stopped]
    );
}

#[test]
fn cloud_recording_limit_is_capture_owned_and_route_neutral() {
    let capture_source = include_str!("../capture.rs");
    assert!(capture_source.contains("CaptureStart::CloudCompleted"));
    assert!(capture_source.contains("CaptureStart::CloudLive"));
    assert!(capture_source.contains("CaptureDestination::CloudLive(feed), recording_limit"));

    let provider_sources = [
        include_str!("../../transcription/cloud/mod.rs"),
        include_str!("../../transcription/cloud/completed.rs"),
    ]
    .concat();
    for forbidden in [
        ["cloud_", "recording_limit_secs"].concat(),
        ["spawn_cloud_", "recording_limit"].concat(),
        ["CloudLimit", "Timer"].concat(),
    ] {
        assert!(
            !provider_sources.contains(&forbidden),
            "provider transport must not own a competing recording interaction timer: {forbidden}"
        );
    }
}

#[test]
fn stale_start_generation_cannot_disarm_newer_cloud_recording_limit() {
    use std::sync::Barrier;

    let (engine, rx) = AudioEngine::new(|_| {});
    let old_id = RecordingIdentity(71);
    let new_id = RecordingIdentity(72);
    let first_guard_entered = Arc::new(Barrier::new(2));
    let release_first_guard = Arc::new(Barrier::new(2));
    let old_guard_calls = Arc::new(AtomicUsize::new(0));

    let old_engine = Arc::clone(&engine);
    let old_thread = std::thread::spawn({
        let first_guard_entered = Arc::clone(&first_guard_entered);
        let release_first_guard = Arc::clone(&release_first_guard);
        let old_guard_calls = Arc::clone(&old_guard_calls);
        move || {
            let start_allowed = Arc::new(move || {
                if old_guard_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    first_guard_entered.wait();
                    release_first_guard.wait();
                    true
                } else {
                    false
                }
            }) as StartGuard;
            old_engine.start_cloud_completed_with_identity(
                old_id,
                Duration::from_secs(30),
                start_allowed,
                |_, generation, recording_id, destination| {
                    Some(test_session_for(
                        recording_id,
                        generation,
                        true,
                        destination,
                        vec![0.25; 8],
                    ))
                },
            )
        }
    });

    first_guard_entered.wait();
    assert!(engine.start_cloud_completed_with_identity(
        new_id,
        Duration::from_millis(50),
        Arc::new(|| true),
        |_, generation, recording_id, destination| {
            Some(test_session_for(
                recording_id,
                generation,
                true,
                destination,
                vec![0.75; 8],
            ))
        },
    ));

    release_first_guard.wait();
    assert!(!old_thread.join().unwrap());

    wait_until(|| !engine.is_active());
    let captured = rx.recv_timeout(Duration::from_millis(100)).unwrap();
    assert_eq!(captured.recording_id, new_id);
    assert_eq!(captured.samples, vec![0.75; 8]);
}

#[test]
fn stale_candidate_start_cannot_disarm_newer_live_cloud_recording_limit() {
    use std::sync::Barrier;

    let (engine, rx) = AudioEngine::new(|_| {});
    let old_id = RecordingIdentity(73);
    let live_id = RecordingIdentity(74);
    let first_guard_entered = Arc::new(Barrier::new(2));
    let release_first_guard = Arc::new(Barrier::new(2));
    let old_guard_calls = Arc::new(AtomicUsize::new(0));

    let old_engine = Arc::clone(&engine);
    let old_thread = std::thread::spawn({
        let first_guard_entered = Arc::clone(&first_guard_entered);
        let release_first_guard = Arc::clone(&release_first_guard);
        let old_guard_calls = Arc::clone(&old_guard_calls);
        move || {
            let start_allowed = Arc::new(move || {
                if old_guard_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    first_guard_entered.wait();
                    release_first_guard.wait();
                    true
                } else {
                    false
                }
            }) as StartGuard;
            old_engine.start_cloud_completed_with_identity(
                old_id,
                Duration::from_secs(30),
                start_allowed,
                |_, generation, recording_id, destination| {
                    Some(test_session_for(
                        recording_id,
                        generation,
                        true,
                        destination,
                        vec![0.25; 8],
                    ))
                },
            )
        }
    });

    first_guard_entered.wait();
    let feed_events = Arc::new(Mutex::new(Vec::new()));
    let stopped = Arc::clone(&feed_events);
    let feed = CloudLiveFeed::new(
        |_| Ok(()),
        |_| Ok(()),
        move || {
            stopped.lock().push("stop".to_string());
            Ok(())
        },
        || {},
    );
    assert!(engine.start_cloud_live_with_identity(
        live_id,
        feed,
        Some(Duration::from_millis(50)),
        Arc::new(|| true),
        |_, generation, recording_id, destination| {
            Some(test_session_for(
                recording_id,
                generation,
                true,
                destination,
                Vec::new(),
            ))
        },
    ));

    release_first_guard.wait();
    assert!(!old_thread.join().unwrap());

    wait_until(|| !engine.is_active());
    assert_eq!(&*feed_events.lock(), &["stop"]);
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
}

#[test]
fn cloud_live_untimed_recording_limit_does_not_spawn_timer_and_requires_manual_stop() {
    let feed_events = Arc::new(Mutex::new(Vec::new()));
    let stopped = Arc::clone(&feed_events);
    let feed = CloudLiveFeed::new(
        |_| Ok(()),
        |_| Ok(()),
        move || {
            stopped.lock().push("stop".to_string());
            Ok(())
        },
        || {},
    );
    let (engine, _rx) = AudioEngine::new(|_| {});
    let live_id = RecordingIdentity(920);
    assert!(engine.start_cloud_live_with_identity(
        live_id,
        feed,
        None,
        Arc::new(|| true),
        |_, generation, recording_id, destination| {
            Some(test_session_for(
                recording_id,
                generation,
                true,
                destination,
                Vec::new(),
            ))
        },
    ));

    // Verify it stays active without timer expiration
    std::thread::sleep(Duration::from_millis(50));
    assert!(engine.is_active());
    assert!(feed_events.lock().is_empty());

    // Manual stop terminates the session
    assert!(engine.stop_for(live_id));
    assert!(!engine.is_active());
    assert_eq!(&*feed_events.lock(), &["stop"]);
}

#[test]
fn cloud_recording_limit_matches_manual_stop_delivery_and_state_contract() {
    fn run(manual: bool) -> (RecordingIdentity, Vec<f32>, Vec<CaptureEvent>) {
        let states = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&states);
        let (engine, rx) = AudioEngine::new(move |state| observed.lock().push(state));
        let expected = vec![0.1, 0.2, 0.3, 0.4];

        assert!(engine.start_cloud_completed_with(
            Duration::from_millis(30),
            |_, generation, destination| {
                Some(test_session(
                    generation,
                    true,
                    destination,
                    expected.clone(),
                ))
            }
        ));
        if manual {
            engine.stop();
        } else {
            wait_until(|| !engine.is_active());
        }

        let captured = rx.recv_timeout(Duration::from_millis(100)).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        let state_history = states.lock().clone();
        (captured.recording_id, captured.samples, state_history)
    }

    let manual = run(true);
    let automatic = run(false);
    assert_eq!(manual, automatic);
    assert_eq!(manual.0, RecordingIdentity(1));
    assert_eq!(manual.1, vec![0.1, 0.2, 0.3, 0.4]);
    assert_eq!(manual.2, vec![CaptureEvent::Started, CaptureEvent::Stopped]);
}

/// Live capture path: start → real samples → stop → delivery.
#[test]
#[ignore = "requires a microphone"]
fn start_stop_delivers_buffer() {
    let recording = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&recording);
    let (engine, rx) = AudioEngine::new(move |event| {
        flag.store(matches!(event, CaptureEvent::Started), Ordering::SeqCst)
    });

    assert!(engine.start_cloud_completed(Duration::from_secs(30)));
    assert!(engine.is_active());
    assert!(recording.load(Ordering::SeqCst));
    std::thread::sleep(Duration::from_millis(600));
    engine.stop();

    assert!(!engine.is_active());
    assert!(!recording.load(Ordering::SeqCst));
    let captured = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        !captured.samples.is_empty(),
        "a 600 ms capture must produce samples"
    );
    assert!(captured.sample_rate >= 8_000);
    assert!(captured
        .samples
        .iter()
        .all(|s| s.is_finite() && s.abs() <= 1.0));
    // ~600 ms of audio at the device rate, ±50 ms of scheduling slack.
    let expected = captured.sample_rate as f32 * 0.6;
    assert!(
        (captured.samples.len() as f32 - expected).abs() < captured.sample_rate as f32 * 0.05,
        "captured {} samples, expected ~{expected}",
        captured.samples.len()
    );
    // The second session gets a fresh buffer — no cross-session reuse.
    assert!(engine.start_cloud_completed(Duration::from_secs(30)));
    std::thread::sleep(Duration::from_millis(100));
    engine.stop();
    let second = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(second.samples.len() < captured.samples.len());
}

/// The Cloud recording limit stops capture at the configured bound and delivers the buffer.
#[test]
#[ignore = "requires a microphone"]
fn cloud_recording_limit_fires_within_bounds() {
    let (engine, rx) = AudioEngine::new(|_| {});
    assert!(engine.start_cloud_completed(Duration::from_secs(1)));
    assert!(engine.is_active());
    std::thread::sleep(Duration::from_millis(1600));
    assert!(
        !engine.is_active(),
        "Cloud recording limit must have fired by 1.6 s"
    );
    let captured = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(!captured.samples.is_empty());
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}

/// Duplicate start is a no-op; a single stop ends the session once.
#[test]
#[ignore = "requires a microphone"]
fn start_is_idempotent() {
    let (engine, rx) = AudioEngine::new(|_| {});
    assert!(engine.start_cloud_completed(Duration::from_secs(30)));
    assert!(engine.start_cloud_completed(Duration::from_secs(30)));
    std::thread::sleep(Duration::from_millis(150));
    engine.stop();
    assert!(!engine.is_active());
    let captured = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(!captured.samples.is_empty());
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}
