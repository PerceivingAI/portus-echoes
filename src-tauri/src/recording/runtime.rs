//! Application orchestration for recording begin/stop/abort and exact-ID effects.

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager};

use crate::app_state::AppState;
use crate::audio::{AudioEngine, CaptureEvent, CaptureEventKind, StopCaptureResult};
use crate::models::{CloudRoute, ProviderId, RecordingIdentity, RecordingPhase};
use crate::transcription::local::{
    LocalEngine, LocalProcessingControl, LocalProcessingLease, LocalRecordingConfig,
    PreparedLocalRecording, ReadyLocalModel,
};

use super::coordinator::RecordingCoordinator;
use super::processing::{
    claim_cloud_completed_job_exact, register_live_cloud_processing_exact, request_abort_all,
    wait_for_cleanup_all, CloudProcessingControl, CloudProcessingRegistry, LocalProcessingRegistry,
};

/// Run one user-visible/current-generation effect at the coordinator's
/// authority linearization point. A newer shortcut press cannot become
/// authoritative between the identity check and this effect.
pub(crate) fn run_if_authoritative<R>(
    app: &AppHandle,
    id: RecordingIdentity,
    action: impl FnOnce() -> R,
) -> Option<R> {
    app.state::<RecordingCoordinator>()
        .run_if_authoritative(id, action)
}

fn emit_failure_if_authoritative(
    app: &AppHandle,
    id: RecordingIdentity,
    error: crate::transcription::TranscriptionError,
) -> bool {
    app.state::<RecordingCoordinator>()
        .claim_failure_with(id, || crate::app::emit_transcription_error(app, error))
}

fn release_matching(app: &AppHandle, id: RecordingIdentity) {
    app.state::<RecordingCoordinator>()
        .complete_if_matching_with(id, || {
            app.state::<AppState>()
                .set_recording_phase(app, RecordingPhase::Idle);
        });
    app.state::<crate::hotkey::HotkeyEngine>().reset_hold_state();
}

fn cancel_superseded(app: &AppHandle, id: RecordingIdentity) {
    app.state::<CloudProcessingRegistry>().cancel(id);
    app.state::<LocalProcessingRegistry>().request_abort(id);
    app.state::<Arc<AudioEngine>>().abort_async(id);
}
pub fn abort_active_recording(app: &AppHandle) {
    let coordinator = app.state::<RecordingCoordinator>();
    if let Some(id) = coordinator.active_identity() {
        app.state::<CloudProcessingRegistry>().cancel(id);
        app.state::<LocalProcessingRegistry>().request_abort(id);
        app.state::<Arc<AudioEngine>>().abort_for(id);
        release_matching(app, id);
    }
    crate::app::windows::cancel_overlay_hide();
    crate::app::windows::hide_overlay(app);
    let _ = app.emit(
        crate::models::DELIVERY_CLIPBOARD_STATUS_EVENT,
        crate::models::ClipboardStatusPayload {
            status: crate::models::ClipboardDeliveryStatus::Idle,
        },
    );
}

fn supersede_then_resolve_local<T>(
    coordinator: &RecordingCoordinator,
    provider: ProviderId,
    cloud_route: Option<CloudRoute>,
    on_commit: impl FnOnce(RecordingIdentity),
    on_superseded: impl FnOnce(RecordingIdentity),
    resolve_local: impl FnOnce(RecordingIdentity) -> Option<T>,
) -> (RecordingIdentity, Option<T>) {
    let (id, superseded) = coordinator.supersede_with(provider, cloud_route, on_commit);
    if let Some(old_id) = superseded {
        on_superseded(old_id);
    }
    let local = match provider {
        ProviderId::Local => resolve_local(id),
        ProviderId::Openai | ProviderId::Groq => None,
    };
    (id, local)
}

fn register_then_resolve_local<T>(
    registry: &LocalProcessingRegistry,
    id: RecordingIdentity,
    control: LocalProcessingControl,
    lifecycle: LocalProcessingLease,
    config: LocalRecordingConfig,
    resolve: impl FnOnce(&LocalRecordingConfig) -> Option<T>,
) -> Option<(T, LocalProcessingLease, LocalRecordingConfig)> {
    if !registry.register(id, control) {
        return None;
    }
    Some((resolve(&config)?, lifecycle, config))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordingCaptureBranch {
    Local,
    CloudCompleted,
    CloudLive,
}

fn recording_capture_branch(
    provider: ProviderId,
    cloud_route: Option<CloudRoute>,
) -> Option<RecordingCaptureBranch> {
    match (provider, cloud_route) {
        (ProviderId::Local, None) => Some(RecordingCaptureBranch::Local),
        (ProviderId::Openai | ProviderId::Groq, Some(CloudRoute::CompletedAudio)) => {
            Some(RecordingCaptureBranch::CloudCompleted)
        }
        (ProviderId::Openai | ProviderId::Groq, Some(CloudRoute::LiveAudio)) => {
            Some(RecordingCaptureBranch::CloudLive)
        }
        (ProviderId::Local, Some(_)) | (ProviderId::Openai | ProviderId::Groq, None) => None,
    }
}

struct LiveCloudStart {
    provider: ProviderId,
    model: String,
    control: CloudProcessingControl,
}

/// Establish exact-ID live Cloud cancellation ownership synchronously, before
/// the asynchronous preparation path is allowed to construct any live
/// transport/feed. The registry stores a clone of the same cancellation
/// control carried forward by the start path.
fn register_live_cloud_start(
    coordinator: &RecordingCoordinator,
    registry: &CloudProcessingRegistry,
    id: RecordingIdentity,
    model: String,
) -> Option<LiveCloudStart> {
    let control = CloudProcessingControl::new();
    let provider =
        register_live_cloud_processing_exact(coordinator, registry, id, control.clone())?;
    Some(LiveCloudStart {
        provider,
        model,
        control,
    })
}

fn finish_cancelled_start(app: &AppHandle, id: RecordingIdentity) {
    if app.state::<RecordingCoordinator>().is_authoritative(id) {
        release_matching(app, id);
    } else {
        app.state::<crate::hotkey::HotkeyEngine>().reset_hold_state();
    }
}

fn finish_cancelled_live_start(app: &AppHandle, id: RecordingIdentity) {
    // Removal + cancellation are exact and idempotent. If supersession already
    // won, its cancellation removed this entry first and this becomes a no-op.
    app.state::<CloudProcessingRegistry>().cancel(id);
    finish_cancelled_start(app, id);
}

fn start_recording_async(
    app: AppHandle,
    id: RecordingIdentity,
    provider: ProviderId,
    local_start: Option<(ReadyLocalModel, LocalProcessingLease, LocalRecordingConfig)>,
    live_cloud_start: Option<LiveCloudStart>,
) {
    if !app.state::<RecordingCoordinator>().is_startable(id) {
        if live_cloud_start.is_some() {
            finish_cancelled_live_start(&app, id);
        } else {
            finish_cancelled_start(&app, id);
        }
        return;
    }

    let mic_status = crate::audio::check_microphone_status();
    if mic_status != crate::audio::MicrophoneStatus::Available {
        if live_cloud_start.is_some() {
            app.state::<CloudProcessingRegistry>().cancel(id);
        }
        if app.state::<RecordingCoordinator>().is_startable(id) {
            app.state::<AppState>()
                .set_recording_phase(&app, RecordingPhase::Muted);
        } else if live_cloud_start.is_some() {
            finish_cancelled_live_start(&app, id);
        } else {
            finish_cancelled_start(&app, id);
        }
        return;
    }

    let frozen_cloud_route = match provider {
        ProviderId::Local => None,
        ProviderId::Openai | ProviderId::Groq => {
            app.state::<RecordingCoordinator>().cloud_route_for(id)
        }
    };
    let Some(branch) = recording_capture_branch(provider, frozen_cloud_route) else {
        if live_cloud_start.is_some() {
            finish_cancelled_live_start(&app, id);
        } else {
            finish_cancelled_start(&app, id);
        }
        return;
    };

    match branch {
        RecordingCaptureBranch::Local => {
            let Some((local_model, lifecycle, config)) = local_start else {
                finish_cancelled_start(&app, id);
                return;
            };
            let prepared = match PreparedLocalRecording::prepare(&app, local_model, config) {
                Ok(prepared) => prepared,
                Err(error) => {
                    app.state::<RecordingCoordinator>()
                        .claim_startable_failure_with(id, || {
                            crate::app::emit_transcription_error(&app, error.into());
                        });
                    finish_cancelled_start(&app, id);
                    return;
                }
            };

            if !app.state::<RecordingCoordinator>().is_startable(id) {
                finish_cancelled_start(&app, id);
                return;
            }

            let completion_app = app.clone();
            let feed = prepared.into_audio_feed(lifecycle, app.clone(), id, move |result| {
                if let Err(error) = &result {
                    emit_failure_if_authoritative(&completion_app, id, (*error).into());
                    completion_app.state::<Arc<AudioEngine>>().abort_for(id);
                    completion_app.state::<LocalEngine>().recover_active_model();
                }
                release_matching(&completion_app, id);
            });

            if !app.state::<RecordingCoordinator>().is_startable(id) {
                app.state::<LocalProcessingRegistry>().request_abort(id);
                finish_cancelled_start(&app, id);
                return;
            }

            let start_guard = {
                let guard_app = app.clone();
                Arc::new(move || guard_app.state::<RecordingCoordinator>().is_startable(id))
                    as Arc<dyn Fn() -> bool + Send + Sync>
            };
            let started = app
                .state::<Arc<AudioEngine>>()
                .start_local_for(id, feed, start_guard);
            if !started {
                app.state::<LocalProcessingRegistry>().request_abort(id);
                if app.state::<RecordingCoordinator>().is_startable(id) {
                    app.state::<AppState>()
                        .set_recording_phase(&app, RecordingPhase::Muted);
                } else {
                    finish_cancelled_start(&app, id);
                }
            } else if let Some(limit) = app.state::<AppState>().recording_limit() {
                let timer_app = app.clone();
                std::thread::spawn(move || {
                    let deadline = std::time::Instant::now() + limit;
                    while std::time::Instant::now() < deadline {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        let coordinator = timer_app.state::<RecordingCoordinator>();
                        if coordinator.active_identity() != Some(id) {
                            return;
                        }
                    }
                    let coordinator = timer_app.state::<RecordingCoordinator>();
                    if coordinator.active_identity() == Some(id) {
                        crate::recording::stop_recording(&timer_app);
                    }
                });
            }
        }
        RecordingCaptureBranch::CloudCompleted => {
            if live_cloud_start.is_some() {
                finish_cancelled_live_start(&app, id);
                return;
            }
            let start_guard = {
                let guard_app = app.clone();
                Arc::new(move || guard_app.state::<RecordingCoordinator>().is_startable(id))
                    as Arc<dyn Fn() -> bool + Send + Sync>
            };
            let recording_limit = app.state::<AppState>().recording_limit();
            let started = app.state::<Arc<AudioEngine>>().start_cloud_completed_for(
                id,
                recording_limit,
                start_guard,
            );
            if !started {
                if app.state::<RecordingCoordinator>().is_startable(id) {
                    app.state::<AppState>()
                        .set_recording_phase(&app, RecordingPhase::Muted);
                } else {
                    finish_cancelled_start(&app, id);
                }
            }
        }
        RecordingCaptureBranch::CloudLive => {
            let Some(live_start) = live_cloud_start else {
                finish_cancelled_start(&app, id);
                return;
            };
            if live_start.provider != provider
                || live_start.control.cancellation_token().is_cancelled()
            {
                finish_cancelled_live_start(&app, id);
                return;
            }

            if !app.state::<RecordingCoordinator>().is_startable(id) {
                finish_cancelled_live_start(&app, id);
                return;
            }
            let state = app.state::<AppState>();
            let model = live_start.model.clone();
            if model.trim().is_empty() {
                finish_cancelled_live_start(&app, id);
                return;
            }
            let Ok(Some(api_key)) = state.api_key(provider) else {
                finish_cancelled_live_start(&app, id);
                return;
            };

            let (audio_tx, audio_rx) = tokio::sync::mpsc::channel(100);
            let feed = crate::transcription::cloud::live::create_cloud_live_feed(audio_tx);

            let live_app = app.clone();
            let cancellation_token = live_start.control.cancellation_token().clone();
            tauri::async_runtime::spawn(async move {
                crate::transcription::cloud::live::run_live_transcription(
                    live_app,
                    id,
                    api_key,
                    model,
                    audio_rx,
                    cancellation_token,
                )
                .await;
            });

            let start_guard = {
                let guard_app = app.clone();
                Arc::new(move || guard_app.state::<RecordingCoordinator>().is_startable(id))
                    as Arc<dyn Fn() -> bool + Send + Sync>
            };
            let recording_limit = app.state::<AppState>().recording_limit();
            let started = app.state::<Arc<AudioEngine>>().start_cloud_live_for(
                id,
                feed,
                recording_limit,
                start_guard,
            );
            if !started {
                if app.state::<RecordingCoordinator>().is_startable(id) {
                    app.state::<AppState>()
                        .set_recording_phase(&app, RecordingPhase::Muted);
                } else {
                    finish_cancelled_live_start(&app, id);
                }
            }
        }
    }
}

/// Establish a new authoritative push-to-talk generation immediately, then do
/// provider preparation on a worker so hotkey release/new press remains responsive.
pub fn begin_recording(app: &AppHandle) -> bool {
    let (settings, delivery_capability) = app.state::<AppState>().recording_admission_snapshot();
    let Some(provider) = settings.active_provider else {
        return false;
    };
    let cloud_route = match provider {
        ProviderId::Local => None,
        ProviderId::Openai | ProviderId::Groq => {
            let Some(route) = crate::cloud_catalog::recording_route(&settings, provider) else {
                return false;
            };
            Some(route)
        }
    };
    let local_config = (provider == ProviderId::Local)
        .then(|| LocalRecordingConfig::from_settings(&settings, delivery_capability));

    let coordinator = app.state::<RecordingCoordinator>();
    let local_engine = app.state::<LocalEngine>();
    let (id, local_start) = supersede_then_resolve_local(
        &coordinator,
        provider,
        cloud_route,
        |_| {
            app.state::<AppState>()
                .set_recording_phase(app, RecordingPhase::Preparing);
        },
        |old_id| cancel_superseded(app, old_id),
        |id| {
            let config = local_config?;
            let registry = app.state::<LocalProcessingRegistry>();
            let cleanup_app = app.clone();
            let (control, lifecycle) = LocalProcessingControl::new(move || {
                cleanup_app.state::<LocalProcessingRegistry>().complete(id);
            });
            register_then_resolve_local(&registry, id, control, lifecycle, config, |config| {
                local_engine.ready_model(config.model_path())
            })
        },
    );
    let live_cloud_start = if cloud_route == Some(CloudRoute::LiveAudio) {
        let model = match provider {
            ProviderId::Openai => settings.openai_model.clone(),
            ProviderId::Groq => settings.groq_model.clone(),
            ProviderId::Local => String::new(),
        };
        register_live_cloud_start(&coordinator, &app.state::<CloudProcessingRegistry>(), id, model)
    } else {
        None
    };
    drop(local_engine);
    drop(coordinator);

    if provider == ProviderId::Local && local_start.is_none() {
        finish_cancelled_start(app, id);
        return false;
    }
    if cloud_route == Some(CloudRoute::LiveAudio) && live_cloud_start.is_none() {
        finish_cancelled_start(app, id);
        return false;
    }

    let start_app = app.clone();
    std::thread::spawn(move || {
        start_recording_async(start_app, id, provider, local_start, live_cloud_start)
    });
    true
}

/// Mark the current identity released and stop only its capture session. If
/// preparation has not installed a session yet, release ownership immediately;
/// the asynchronous start guard prevents a late capture start.
pub fn stop_recording(app: &AppHandle) {
    let coordinator = app.state::<RecordingCoordinator>();
    let Some(id) = coordinator.active_identity() else {
        return;
    };
    // Release ends the visible push-to-talk interaction at the same ownership
    // linearization point as the release flag. A newer press cannot be hidden
    // by a stale release after it becomes authoritative.
    if !coordinator.mark_released_with(id, || {
        app.state::<AppState>()
            .set_recording_phase(app, RecordingPhase::Finalizing);
    }) {
        return;
    }
    drop(coordinator);

    match app.state::<Arc<AudioEngine>>().stop_result_for(id) {
        StopCaptureResult::Stopped | StopCaptureResult::AlreadyStopping => {}
        StopCaptureResult::NotFound => {
            // LiveAudio owns processing cancellation before capture exists. If
            // release wins during that preparation window, there is no capture to
            // stop/finalize, so the exact early control must be canceled now.
            if app.state::<RecordingCoordinator>().cloud_route_for(id)
                == Some(CloudRoute::LiveAudio)
            {
                app.state::<CloudProcessingRegistry>().cancel(id);
            }
            release_matching(app, id);
        }
    }
}

/// Explicit application shutdown. New Local admission is closed first; all
/// current and superseded Local lifecycles are then aborted before any wait.
pub fn shutdown(app: &AppHandle) {
    let local_processing = app.state::<LocalProcessingRegistry>().begin_shutdown();
    let active_id = app.state::<RecordingCoordinator>().active_identity();

    if let Some(id) = active_id {
        release_matching(app, id);
        app.state::<CloudProcessingRegistry>().cancel(id);
        app.state::<Arc<AudioEngine>>().abort_for(id);
    }
    for (id, _) in &local_processing {
        if Some(*id) != active_id {
            app.state::<Arc<AudioEngine>>().abort_for(*id);
        }
    }

    request_abort_all(&local_processing);
    app.state::<LocalEngine>().shutdown();
    wait_for_cleanup_all(local_processing);
}

pub fn toggle_recording(app: &AppHandle) -> bool {
    let coordinator = app.state::<RecordingCoordinator>();
    if let Some(id) = coordinator.active_identity() {
        if !coordinator.is_released(id) {
            drop(coordinator);
            stop_recording(app);
            return true;
        }
    }
    drop(coordinator);
    begin_recording(app)
}

/// Receive identity-bound capture facts from `AudioEngine`. Stale events are
/// rejected before they can mutate the shared recording lifecycle.
pub fn handle_capture_event(app: &AppHandle, event: CaptureEvent) {
    let id = event.recording_id;
    let coordinator = app.state::<RecordingCoordinator>();

    match event.kind {
        CaptureEventKind::Started => {
            match coordinator.run_if_startable(id, || {
                app.state::<AppState>()
                    .set_recording_phase(app, RecordingPhase::Recording);
            }) {
                Ok(()) => {}
                Err(true) => {
                    drop(coordinator);
                    app.state::<Arc<AudioEngine>>().abort_async(id);
                }
                Err(false) => {}
            }
        }
        CaptureEventKind::Stopped => {
            coordinator.mark_capture_stopped_with(id, || {
                app.state::<AppState>()
                    .set_recording_phase(app, RecordingPhase::Finalizing);
            });
        }
        CaptureEventKind::Aborted => {
            let cloud = coordinator.cloud_route_for(id).is_some();
            drop(coordinator);
            if cloud {
                // Idempotent exact cleanup for an early LiveAudio control.
                // Completed capture normally has no registered control here.
                app.state::<CloudProcessingRegistry>().cancel(id);
            }
            release_matching(app, id);
        }
        CaptureEventKind::Failed(_kind) => {
            if coordinator.cloud_route_for(id).is_some() {
                drop(coordinator);
                // Completed capture has no registry entry yet, so this is a
                // no-op there. Live capture owns an early entry and must cancel
                // it on any terminal capture/feed failure.
                app.state::<CloudProcessingRegistry>().cancel(id);
                release_matching(app, id);
            } else {
                coordinator.claim_failure_for_provider_with(id, ProviderId::Local, || {
                    crate::app::emit_transcription_error(
                        app,
                        crate::transcription::TranscriptionError::LocalEngine,
                    );
                });
                drop(coordinator);
                app.state::<LocalEngine>().recover_active_model();
                release_matching(app, id);
            }
        }
    }
}

/// Admit the exact completed Cloud recording and return only its frozen
/// OpenAI/Groq provider. Stale identities and duplicate admission are rejected
/// before credential, conversion, or transport work begins.
pub(crate) fn claim_completed_cloud_job(
    app: &AppHandle,
    id: RecordingIdentity,
    control: CloudProcessingControl,
) -> Option<ProviderId> {
    let coordinator = app.state::<RecordingCoordinator>();
    let registry = app.state::<CloudProcessingRegistry>();
    claim_cloud_completed_job_exact(&coordinator, &registry, id, control)
}

pub(crate) fn complete_cloud(app: &AppHandle, id: RecordingIdentity) {
    app.state::<CloudProcessingRegistry>().clear(id);
    release_matching(app, id);
}

#[cfg(test)]
mod admission_tests {
    use std::{cell::Cell, sync::Arc};

    use super::*;

    fn local_config(
        model_path: &str,
        capability: crate::output::DeliveryCapability,
    ) -> LocalRecordingConfig {
        let settings = crate::models::AppSettings {
            local_model_path: model_path.to_string(),
            ..Default::default()
        };
        LocalRecordingConfig::from_settings(&settings, capability)
    }

    #[test]
    fn resolved_provider_route_maps_to_exact_capture_branch() {
        assert_eq!(
            recording_capture_branch(ProviderId::Openai, Some(CloudRoute::CompletedAudio)),
            Some(RecordingCaptureBranch::CloudCompleted)
        );
        assert_eq!(
            recording_capture_branch(ProviderId::Openai, Some(CloudRoute::LiveAudio)),
            Some(RecordingCaptureBranch::CloudLive)
        );
        assert_eq!(
            recording_capture_branch(ProviderId::Groq, Some(CloudRoute::CompletedAudio)),
            Some(RecordingCaptureBranch::CloudCompleted)
        );
        assert_eq!(
            recording_capture_branch(ProviderId::Local, None),
            Some(RecordingCaptureBranch::Local)
        );

        assert_eq!(
            recording_capture_branch(ProviderId::Local, Some(CloudRoute::CompletedAudio)),
            None
        );
        assert_eq!(recording_capture_branch(ProviderId::Openai, None), None);
        assert_eq!(recording_capture_branch(ProviderId::Groq, None), None);
    }

    #[test]
    fn shared_routing_owners_have_no_provider_protocol_dependency() {
        let sources = [
            include_str!("runtime.rs"),
            include_str!("coordinator.rs"),
            include_str!("processing.rs"),
            include_str!("../audio/capture.rs"),
            include_str!("../cloud_catalog.rs"),
        ]
        .concat();
        for forbidden in [
            ["req", "west"].concat(),
            ["multi", "part"].concat(),
            ["Web", "Socket"].concat(),
            ["api.openai", ".com"].concat(),
            ["api.groq", ".com"].concat(),
            ["/v1/audio/", "transcriptions"].concat(),
        ] {
            assert!(
                !sources.contains(&forbidden),
                "shared routing/capture ownership must remain provider-protocol-free: {forbidden}"
            );
        }
    }

    #[test]
    fn cloud_route_is_resolved_from_admission_snapshot_and_not_reresolved_async() {
        let source = include_str!("runtime.rs");
        let begin = source
            .split_once("pub fn begin_recording(")
            .expect("begin_recording must exist")
            .1
            .split_once("/// Mark the current identity released")
            .expect("stop_recording must follow begin_recording")
            .0;

        let snapshot = begin
            .find("recording_admission_snapshot()")
            .expect("Cloud routing must begin from the recording admission snapshot");
        let route = begin
            .find("crate::cloud_catalog::recording_route(&settings, provider)")
            .expect("Cloud route must resolve from that captured settings snapshot");
        let admission = begin
            .find("supersede_then_resolve_local(")
            .expect("resolved route must be committed with recording admission");
        assert!(snapshot < route && route < admission);
        assert!(begin.contains("provider,\n        cloud_route,"));

        let async_start = source
            .split_once("fn start_recording_async(")
            .expect("start_recording_async must exist")
            .1
            .split_once("/// Establish a new authoritative push-to-talk generation")
            .expect("begin_recording must follow start_recording_async")
            .0;
        assert!(
            async_start.contains("cloud_route_for(id)"),
            "async capture selection must consume the route frozen with RecordingIdentity"
        );
        for forbidden in [
            "recording_admission_snapshot",
            "settings_snapshot",
            "cloud_catalog::recording_route",
        ] {
            assert!(
                !async_start.contains(forbidden),
                "async capture start must not re-resolve Cloud routing from ambient Settings: {forbidden}"
            );
        }
    }

    #[test]
    fn cloud_capture_failure_branch_cancels_processing_before_exact_release() {
        let source = include_str!("runtime.rs");
        let failure = source
            .split_once("CaptureEventKind::Failed(_kind) => {")
            .expect("Cloud capture failure branch must exist")
            .1
            .split_once("} else {")
            .expect("Cloud and Local failure handling must remain distinct")
            .0;

        assert!(failure.contains("coordinator.cloud_route_for(id).is_some()"));
        let cancel = failure
            .find("state::<CloudProcessingRegistry>().cancel(id)")
            .expect("matching Cloud capture failure must cancel exact processing ownership");
        let release = failure
            .find("release_matching(app, id)")
            .expect("matching Cloud capture failure must release only the exact recording");
        assert!(
            cancel < release,
            "Cloud failure cleanup must cancel exact processing before releasing terminal authority"
        );
    }

    #[test]
    fn unready_local_admission_supersedes_before_ready_resolution() {
        let coordinator = RecordingCoordinator::new();
        let (first, _) = coordinator.supersede(ProviderId::Local, None);
        let cancelled = Cell::new(false);

        let engine = LocalEngine::new();
        let (second, model) = supersede_then_resolve_local(
            &coordinator,
            ProviderId::Local,
            None,
            |_| {},
            |superseded| {
                assert_eq!(superseded, first);
                assert!(!coordinator.is_authoritative(first));
                cancelled.set(true);
            },
            |_| {
                assert!(cancelled.get());
                assert_eq!(
                    coordinator.active_identity(),
                    Some(RecordingIdentity(first.0 + 1))
                );
                engine.ready_model(std::path::Path::new("unready.bin"))
            },
        );

        assert_eq!(second, RecordingIdentity(first.0 + 1));
        assert!(model.is_none());
        assert!(coordinator.is_authoritative(second));
        assert_eq!(engine.cached_path(), None);
        assert!(!coordinator.is_authoritative(first));
    }

    #[test]
    fn live_cloud_processing_is_registered_before_async_preparation_boundary() {
        let coordinator = RecordingCoordinator::new();
        let registry = CloudProcessingRegistry::new();
        let (id, _) = coordinator.supersede(ProviderId::Openai, Some(CloudRoute::LiveAudio));

        let start = register_live_cloud_start(&coordinator, &registry, id, "gpt-live-transcribe".to_string())
            .expect("startable LiveAudio must acquire exact processing ownership");

        assert_eq!(start.provider, ProviderId::Openai);
        assert!(registry.contains(id));
        assert!(!start.control.is_cancelled());
    }

    #[test]
    fn local_lifecycle_is_registered_before_ready_resolution() {
        let registry = Arc::new(LocalProcessingRegistry::new());
        let id = RecordingIdentity(1);
        let cleanup_registry = Arc::clone(&registry);
        let (control, lifecycle) = LocalProcessingControl::new(move || {
            cleanup_registry.complete(id);
        });

        let resolved = register_then_resolve_local(
            &registry,
            id,
            control,
            lifecycle,
            local_config("model.bin", crate::output::DeliveryCapability::Inject),
            |_| {
                assert!(registry.contains(id));
                Some(())
            },
        );

        assert!(resolved.is_some());
        drop(resolved);
        assert!(!registry.contains(id));
    }

    #[test]
    fn closed_local_registry_rejects_before_ready_resolution() {
        let registry = Arc::new(LocalProcessingRegistry::new());
        assert!(registry.begin_shutdown().is_empty());
        let id = RecordingIdentity(1);
        let cleanup_registry = Arc::clone(&registry);
        let (control, lifecycle) = LocalProcessingControl::new(move || {
            cleanup_registry.complete(id);
        });
        let resolver_called = Cell::new(false);

        let resolved = register_then_resolve_local(
            &registry,
            id,
            control,
            lifecycle,
            local_config("model.bin", crate::output::DeliveryCapability::Inject),
            |_| {
                resolver_called.set(true);
                Some(())
            },
        );

        assert!(resolved.is_none());
        assert!(!resolver_called.get());
    }

    #[test]
    fn inject_policy_remains_frozen_when_capability_changes_before_resolution() {
        let registry = Arc::new(LocalProcessingRegistry::new());
        let id = RecordingIdentity(1);
        let cleanup_registry = Arc::clone(&registry);
        let (control, lifecycle) = LocalProcessingControl::new(move || {
            cleanup_registry.complete(id);
        });
        let current_capability = Cell::new(crate::output::DeliveryCapability::Inject);
        let expected = local_config("model-a.bin", current_capability.get());

        let (_, lifecycle, frozen) = register_then_resolve_local(
            &registry,
            id,
            control,
            lifecycle,
            expected.clone(),
            |config| {
                current_capability.set(crate::output::DeliveryCapability::Clipboard);
                assert_eq!(config, &expected);
                Some(())
            },
        )
        .expect("open registry must admit the frozen Local config");

        assert_eq!(frozen, expected);
        drop(lifecycle);
    }

    #[test]
    fn clipboard_policy_remains_frozen_when_capability_changes_before_resolution() {
        let registry = Arc::new(LocalProcessingRegistry::new());
        let id = RecordingIdentity(1);
        let cleanup_registry = Arc::clone(&registry);
        let (control, lifecycle) = LocalProcessingControl::new(move || {
            cleanup_registry.complete(id);
        });
        let current_capability = Cell::new(crate::output::DeliveryCapability::Clipboard);
        let expected = local_config("model-a.bin", current_capability.get());

        let (_, lifecycle, frozen) = register_then_resolve_local(
            &registry,
            id,
            control,
            lifecycle,
            expected.clone(),
            |config| {
                current_capability.set(crate::output::DeliveryCapability::Inject);
                assert_eq!(config, &expected);
                Some(())
            },
        )
        .expect("open registry must admit the frozen Local config");

        assert_eq!(frozen, expected);
        drop(lifecycle);
    }

    #[test]
    fn model_path_and_output_share_snapshot_while_later_recording_observes_changes() {
        let first = local_config("model-a.bin", crate::output::DeliveryCapability::Inject);
        let later = local_config("model-b.bin", crate::output::DeliveryCapability::Clipboard);

        assert_eq!(first.model_path(), std::path::Path::new("model-a.bin"));
        assert_eq!(later.model_path(), std::path::Path::new("model-b.bin"));
        assert_eq!(
            first,
            local_config("model-a.bin", crate::output::DeliveryCapability::Inject)
        );
        assert_eq!(
            later,
            local_config("model-b.bin", crate::output::DeliveryCapability::Clipboard)
        );
        assert_ne!(first, later);
    }
}
