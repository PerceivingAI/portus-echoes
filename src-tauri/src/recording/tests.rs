use super::*;
use crate::models::{CloudRoute, ProviderId, RecordingIdentity};

fn completed_route(provider: ProviderId) -> Option<CloudRoute> {
    match provider {
        ProviderId::Local => None,
        ProviderId::Openai | ProviderId::Groq => Some(CloudRoute::CompletedAudio),
    }
}
use std::sync::Arc;

#[test]
fn second_press_supersedes_immediately_with_monotonic_identity() {
    let coordinator = RecordingCoordinator::new();
    let (first, replaced) = coordinator.supersede(ProviderId::Local, None);
    assert_eq!(first, RecordingIdentity(1));
    assert_eq!(replaced, None);

    let (second, replaced) =
        coordinator.supersede(ProviderId::Openai, Some(CloudRoute::CompletedAudio));
    assert_eq!(second, RecordingIdentity(2));
    assert_eq!(replaced, Some(first));
    assert!(!coordinator.is_authoritative(first));
    assert!(coordinator.is_authoritative(second));
}

#[test]
fn cloud_route_is_frozen_with_the_recording_identity() {
    let coordinator = RecordingCoordinator::new();
    let (first, _) = coordinator.supersede(ProviderId::Openai, Some(CloudRoute::LiveAudio));
    assert_eq!(
        coordinator.cloud_route_for(first),
        Some(CloudRoute::LiveAudio)
    );

    let (second, replaced) =
        coordinator.supersede(ProviderId::Openai, Some(CloudRoute::CompletedAudio));
    assert_eq!(replaced, Some(first));
    assert_eq!(coordinator.cloud_route_for(first), None);
    assert_eq!(
        coordinator.cloud_route_for(second),
        Some(CloudRoute::CompletedAudio)
    );
}

#[test]
#[should_panic(expected = "recording admission must carry exactly one Cloud route")]
fn cloud_admission_without_a_route_is_rejected() {
    let coordinator = RecordingCoordinator::new();
    let _ = coordinator.supersede(ProviderId::Openai, None);
}

#[test]
fn concurrent_supersession_assigns_identity_in_authority_commit_order() {
    use std::sync::{Barrier, Mutex as StdMutex};

    const STARTS: usize = 32;
    let coordinator = Arc::new(RecordingCoordinator::new());
    let barrier = Arc::new(Barrier::new(STARTS));
    let committed = Arc::new(StdMutex::new(Vec::with_capacity(STARTS)));
    let mut workers = Vec::with_capacity(STARTS);

    for _ in 0..STARTS {
        let coordinator = Arc::clone(&coordinator);
        let barrier = Arc::clone(&barrier);
        let committed = Arc::clone(&committed);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            coordinator.supersede_with(ProviderId::Local, None, |id| {
                committed.lock().unwrap().push(id);
            })
        }));
    }

    for worker in workers {
        worker.join().unwrap();
    }

    let committed = committed.lock().unwrap();
    assert_eq!(committed.len(), STARTS);
    for (index, id) in committed.iter().enumerate() {
        assert_eq!(*id, RecordingIdentity(index as u64 + 1));
    }
    assert_eq!(coordinator.active_identity(), committed.last().copied());
}

#[test]
fn authoritative_effect_and_new_supersession_are_linearized() {
    use std::sync::mpsc;
    use std::time::Duration;

    let coordinator = Arc::new(RecordingCoordinator::new());
    let (first, _) = coordinator.supersede(ProviderId::Local, None);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    let effect_coordinator = Arc::clone(&coordinator);
    let effect = std::thread::spawn(move || {
        effect_coordinator
            .run_if_authoritative(first, || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
            .unwrap();
    });
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let (attempt_tx, attempt_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let supersede_coordinator = Arc::clone(&coordinator);
    let supersede = std::thread::spawn(move || {
        attempt_tx.send(()).unwrap();
        let result =
            supersede_coordinator.supersede(ProviderId::Openai, Some(CloudRoute::CompletedAudio));
        result_tx.send(result).unwrap();
    });
    attempt_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(
        result_rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "a newer identity must not interleave inside an already-authorized effect"
    );

    release_tx.send(()).unwrap();
    effect.join().unwrap();
    let (second, replaced) = result_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    supersede.join().unwrap();

    assert_eq!(replaced, Some(first));
    assert_eq!(second, RecordingIdentity(2));
    assert!(coordinator.is_authoritative(second));
}

#[test]
fn stale_callbacks_cannot_commit_effects_after_supersession() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let coordinator = RecordingCoordinator::new();
    let (first, _) = coordinator.supersede(ProviderId::Local, None);
    let (_second, _) = coordinator.supersede(ProviderId::Openai, Some(CloudRoute::CompletedAudio));
    let effects = AtomicUsize::new(0);

    assert!(!coordinator.complete_if_matching_with(first, || {
        effects.fetch_add(1, Ordering::SeqCst);
    }));
    assert!(!coordinator.claim_failure_with(first, || {
        effects.fetch_add(1, Ordering::SeqCst);
    }));
    assert!(coordinator
        .run_if_authoritative(first, || {
            effects.fetch_add(1, Ordering::SeqCst);
        })
        .is_none());
    assert_eq!(
        coordinator.run_if_startable(first, || {
            effects.fetch_add(1, Ordering::SeqCst);
        }),
        Err(false)
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[test]
fn stale_completion_cannot_release_a_newer_recording() {
    let coordinator = RecordingCoordinator::new();
    let (first, _) = coordinator.supersede(ProviderId::Local, None);
    let (second, _) = coordinator.supersede(ProviderId::Openai, Some(CloudRoute::CompletedAudio));
    assert!(!coordinator.complete_if_matching(first));
    assert_eq!(coordinator.active_identity(), Some(second));
}

#[test]
fn stale_failure_cannot_be_claimed_against_new_identity() {
    let coordinator = RecordingCoordinator::new();
    let (first, _) = coordinator.supersede(ProviderId::Local, None);
    let (second, _) = coordinator.supersede(ProviderId::Local, None);
    assert!(!coordinator.claim_failure(first));
    assert!(coordinator.claim_failure(second));
    assert!(!coordinator.claim_failure(second));
}

#[test]
fn release_during_preparation_revokes_start_permission() {
    let coordinator = RecordingCoordinator::new();
    let (id, _) = coordinator.supersede(ProviderId::Local, None);
    assert!(coordinator.is_startable(id));
    assert!(coordinator.mark_released(id));
    assert!(!coordinator.is_startable(id));
    assert!(coordinator.is_authoritative(id));
}

#[test]
fn cloud_capture_stop_closes_start_permission_without_releasing_terminal_authority() {
    for route in [CloudRoute::CompletedAudio, CloudRoute::LiveAudio] {
        let coordinator = RecordingCoordinator::new();
        let (id, _) = coordinator.supersede(ProviderId::Openai, Some(route));
        assert!(coordinator.is_startable(id));

        assert!(coordinator.mark_capture_stopped_with(id, || {}));
        assert!(coordinator.is_authoritative(id));
        assert!(coordinator.is_released(id));
        assert!(!coordinator.is_startable(id));

        // A later physical hotkey release after an automatic Cloud-limit stop
        // must not clear authority while the exact Cloud job is finalizing.
        assert!(!coordinator.mark_released(id));
        assert!(coordinator.is_authoritative(id));
    }
}

#[test]
fn exact_cloud_admission_returns_only_the_originating_frozen_provider() {
    let coordinator = RecordingCoordinator::new();
    let registry = CloudProcessingRegistry::new();
    let (first, _) = coordinator.supersede(ProviderId::Openai, Some(CloudRoute::CompletedAudio));
    let first_control = CloudProcessingControl::new();

    assert_eq!(
        claim_cloud_completed_job_exact(&coordinator, &registry, first, first_control),
        Some(ProviderId::Openai)
    );
    assert!(registry.contains(first));

    let (second, _) = coordinator.supersede(ProviderId::Groq, Some(CloudRoute::CompletedAudio));
    let stale_control = CloudProcessingControl::new();
    assert_eq!(
        claim_cloud_completed_job_exact(&coordinator, &registry, first, stale_control),
        None
    );

    let second_control = CloudProcessingControl::new();
    assert_eq!(
        claim_cloud_completed_job_exact(&coordinator, &registry, second, second_control),
        Some(ProviderId::Groq)
    );
    assert!(registry.contains(second));
}

#[test]
fn completed_cloud_processing_still_registers_after_capture_stop() {
    let coordinator = RecordingCoordinator::new();
    let registry = CloudProcessingRegistry::new();
    let (id, _) = coordinator.supersede(ProviderId::Groq, Some(CloudRoute::CompletedAudio));
    assert!(coordinator.mark_capture_stopped_with(id, || {}));
    assert!(coordinator.is_released(id));
    assert!(!coordinator.is_startable(id));

    let control = CloudProcessingControl::new();
    assert_eq!(
        claim_cloud_completed_job_exact(&coordinator, &registry, id, control.clone()),
        Some(ProviderId::Groq)
    );
    assert!(registry.contains(id));
    assert!(!control.is_cancelled());
}

#[test]
fn live_cloud_processing_registers_only_for_exact_startable_live_route() {
    let coordinator = RecordingCoordinator::new();
    let registry = CloudProcessingRegistry::new();
    let (live_id, _) = coordinator.supersede(ProviderId::Openai, Some(CloudRoute::LiveAudio));
    let live_control = CloudProcessingControl::new();

    assert_eq!(
        register_live_cloud_processing_exact(
            &coordinator,
            &registry,
            live_id,
            live_control.clone(),
        ),
        Some(ProviderId::Openai)
    );
    assert!(registry.contains(live_id));
    assert!(!live_control.is_cancelled());

    let duplicate = CloudProcessingControl::new();
    assert_eq!(
        register_live_cloud_processing_exact(&coordinator, &registry, live_id, duplicate.clone()),
        None
    );
    assert!(!duplicate.is_cancelled());
}

#[test]
fn cloud_routes_cannot_cross_completed_and_live_processing_admission() {
    let coordinator = RecordingCoordinator::new();
    let registry = CloudProcessingRegistry::new();

    let (completed_id, _) =
        coordinator.supersede(ProviderId::Openai, Some(CloudRoute::CompletedAudio));
    assert_eq!(
        register_live_cloud_processing_exact(
            &coordinator,
            &registry,
            completed_id,
            CloudProcessingControl::new(),
        ),
        None
    );
    assert!(!registry.contains(completed_id));

    let (live_id, _) = coordinator.supersede(ProviderId::Groq, Some(CloudRoute::LiveAudio));
    assert_eq!(
        claim_cloud_completed_job_exact(
            &coordinator,
            &registry,
            live_id,
            CloudProcessingControl::new(),
        ),
        None
    );
    assert!(!registry.contains(live_id));
}

#[test]
fn released_or_stale_live_identity_cannot_gain_processing_ownership() {
    let coordinator = RecordingCoordinator::new();
    let registry = CloudProcessingRegistry::new();
    let (released_id, _) = coordinator.supersede(ProviderId::Openai, Some(CloudRoute::LiveAudio));
    assert!(coordinator.mark_released(released_id));
    assert_eq!(
        register_live_cloud_processing_exact(
            &coordinator,
            &registry,
            released_id,
            CloudProcessingControl::new(),
        ),
        None
    );

    let (current_id, _) = coordinator.supersede(ProviderId::Groq, Some(CloudRoute::LiveAudio));
    assert_eq!(
        register_live_cloud_processing_exact(
            &coordinator,
            &registry,
            released_id,
            CloudProcessingControl::new(),
        ),
        None
    );
    assert!(!registry.contains(released_id));
    assert!(coordinator.is_authoritative(current_id));
}

#[test]
fn released_live_preparation_cannot_regain_start_authority() {
    let coordinator = RecordingCoordinator::new();
    let registry = CloudProcessingRegistry::new();
    let (id, _) = coordinator.supersede(ProviderId::Openai, Some(CloudRoute::LiveAudio));
    let control = CloudProcessingControl::new();

    assert_eq!(
        register_live_cloud_processing_exact(&coordinator, &registry, id, control.clone()),
        Some(ProviderId::Openai)
    );
    assert!(registry.contains(id));
    assert!(coordinator.mark_released(id));
    assert!(!coordinator.is_startable(id));
    assert!(
        coordinator
            .with_matching_startable_cloud_route(id, |_, _| ())
            .is_none(),
        "released live preparation must not regain remote/live start authority"
    );

    assert!(registry.cancel(id));
    assert!(control.is_cancelled());
    assert!(!registry.contains(id));
}

#[test]
fn supersession_cancels_early_registered_live_cloud_control_exactly() {
    let coordinator = RecordingCoordinator::new();
    let registry = CloudProcessingRegistry::new();
    let (live_id, _) = coordinator.supersede(ProviderId::Openai, Some(CloudRoute::LiveAudio));
    let live_control = CloudProcessingControl::new();
    assert_eq!(
        register_live_cloud_processing_exact(
            &coordinator,
            &registry,
            live_id,
            live_control.clone(),
        ),
        Some(ProviderId::Openai)
    );

    let (new_id, superseded) =
        coordinator.supersede(ProviderId::Groq, Some(CloudRoute::CompletedAudio));
    assert_eq!(superseded, Some(live_id));
    assert!(registry.cancel(live_id));
    assert!(live_control.is_cancelled());
    assert!(!registry.contains(live_id));
    assert!(coordinator.is_authoritative(new_id));
}

#[test]
fn local_identity_cannot_be_admitted_as_a_cloud_job() {
    let coordinator = RecordingCoordinator::new();
    let registry = CloudProcessingRegistry::new();
    let (id, _) = coordinator.supersede(ProviderId::Local, None);

    assert_eq!(
        claim_cloud_completed_job_exact(&coordinator, &registry, id, CloudProcessingControl::new()),
        None
    );
    assert!(!registry.contains(id));
}

#[test]
fn supersession_before_admission_rejects_the_stale_cloud_buffer() {
    let coordinator = RecordingCoordinator::new();
    let registry = CloudProcessingRegistry::new();
    let (first, _) = coordinator.supersede(ProviderId::Openai, Some(CloudRoute::CompletedAudio));
    let (_second, superseded) =
        coordinator.supersede(ProviderId::Groq, Some(CloudRoute::CompletedAudio));
    assert_eq!(superseded, Some(first));

    let stale_control = CloudProcessingControl::new();
    assert_eq!(
        claim_cloud_completed_job_exact(&coordinator, &registry, first, stale_control.clone()),
        None
    );
    assert!(!registry.contains(first));
    assert!(!stale_control.is_cancelled());
}

#[test]
fn explicit_shutdown_ownership_sequence_revokes_and_cancels_exact_cloud_job() {
    let coordinator = RecordingCoordinator::new();
    let registry = CloudProcessingRegistry::new();
    let (id, _) = coordinator.supersede(ProviderId::Openai, Some(CloudRoute::CompletedAudio));
    let control = CloudProcessingControl::new();

    assert_eq!(
        claim_cloud_completed_job_exact(&coordinator, &registry, id, control.clone()),
        Some(ProviderId::Openai)
    );
    assert!(registry.contains(id));

    assert!(coordinator.complete_if_matching(id));
    assert!(registry.cancel(id));
    assert!(control.is_cancelled());
    assert!(!coordinator.is_authoritative(id));
    assert!(!registry.contains(id));

    let late_control = CloudProcessingControl::new();
    assert_eq!(
        claim_cloud_completed_job_exact(&coordinator, &registry, id, late_control.clone()),
        None
    );
    assert!(!late_control.is_cancelled());
    assert!(!registry.contains(id));
}

#[test]
fn admission_before_supersession_registers_then_cancels_the_old_job() {
    let coordinator = RecordingCoordinator::new();
    let registry = CloudProcessingRegistry::new();
    let (first, _) = coordinator.supersede(ProviderId::Openai, Some(CloudRoute::CompletedAudio));
    let first_control = CloudProcessingControl::new();

    assert_eq!(
        claim_cloud_completed_job_exact(&coordinator, &registry, first, first_control.clone()),
        Some(ProviderId::Openai)
    );
    assert!(registry.contains(first));

    let (second, superseded) =
        coordinator.supersede(ProviderId::Groq, Some(CloudRoute::CompletedAudio));
    assert_eq!(superseded, Some(first));
    assert!(registry.cancel(first));

    assert!(first_control.is_cancelled());
    assert!(!registry.contains(first));
    assert!(coordinator.is_authoritative(second));
}

#[test]
fn superseded_cloud_effects_are_suppressed_for_completed_and_live_results() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    for old_route in [CloudRoute::CompletedAudio, CloudRoute::LiveAudio] {
        for replacement in [ProviderId::Groq, ProviderId::Local] {
            let coordinator = RecordingCoordinator::new();
            let (old_id, _) = coordinator.supersede(ProviderId::Openai, Some(old_route));
            let (new_id, superseded) =
                coordinator.supersede(replacement, completed_route(replacement));
            assert_eq!(superseded, Some(old_id));

            let effects = AtomicUsize::new(0);
            for _ in 0..4 {
                assert!(coordinator
                    .run_if_authoritative(old_id, || {
                        effects.fetch_add(1, Ordering::SeqCst);
                    })
                    .is_none());
            }
            assert!(!coordinator.complete_if_matching_with(old_id, || {
                effects.fetch_add(1, Ordering::SeqCst);
            }));

            assert_eq!(effects.load(Ordering::SeqCst), 0);
            assert_eq!(coordinator.active_identity(), Some(new_id));
        }
    }
}

#[test]
fn repeated_cloud_supersession_cancels_only_old_controls_and_leaves_no_completed_entries() {
    let coordinator = RecordingCoordinator::new();
    let registry = CloudProcessingRegistry::new();

    let (mut current_id, _) =
        coordinator.supersede(ProviderId::Openai, Some(CloudRoute::CompletedAudio));
    let mut current_control = CloudProcessingControl::new();
    assert_eq!(
        claim_cloud_completed_job_exact(
            &coordinator,
            &registry,
            current_id,
            current_control.clone(),
        ),
        Some(ProviderId::Openai)
    );

    for provider in [ProviderId::Groq, ProviderId::Openai, ProviderId::Groq] {
        let old_id = current_id;
        let old_control = current_control.clone();
        let (new_id, superseded) = coordinator.supersede(provider, completed_route(provider));
        assert_eq!(superseded, Some(old_id));
        assert!(registry.cancel(old_id));
        assert!(old_control.is_cancelled());
        assert!(!registry.contains(old_id));

        current_id = new_id;
        current_control = CloudProcessingControl::new();
        assert_eq!(
            claim_cloud_completed_job_exact(
                &coordinator,
                &registry,
                current_id,
                current_control.clone(),
            ),
            Some(provider)
        );
        assert!(registry.contains(current_id));
        assert!(!current_control.is_cancelled());

        assert!(!registry.clear(old_id));
        assert!(registry.contains(current_id));
    }

    assert!(registry.clear(current_id));
    assert!(!registry.contains(current_id));
}

#[test]
fn cloud_registry_operations_are_exact_and_idempotent() {
    let registry = CloudProcessingRegistry::new();
    let first = RecordingIdentity(10);
    let second = RecordingIdentity(11);
    let first_control = CloudProcessingControl::new();
    let second_control = CloudProcessingControl::new();
    let duplicate = CloudProcessingControl::new();

    assert!(registry.register(first, first_control.clone()));
    assert!(registry.register(second, second_control.clone()));
    assert!(!registry.register(first, duplicate.clone()));
    assert!(!duplicate.is_cancelled());

    assert!(registry.cancel(first));
    assert!(!registry.cancel(first));
    assert!(first_control.is_cancelled());
    assert!(!second_control.is_cancelled());
    assert!(!registry.contains(first));
    assert!(registry.contains(second));

    assert!(!registry.clear(first));
    assert!(registry.clear(second));
    assert!(!registry.clear(second));
    assert!(!second_control.is_cancelled());
    assert!(!registry.contains(second));
}
