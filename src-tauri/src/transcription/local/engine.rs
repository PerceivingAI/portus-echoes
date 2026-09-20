use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use whisper_rs::vulkan::{list_devices as list_vulkan_devices, VulkanDeviceInfo, VulkanDeviceKind};
use whisper_rs::{WhisperContext, WhisperContextParameters, WhisperError};

use super::worker::{
    spawn_local_worker, LocalWorkerSpawner, SharedLocalInferenceClient, SharedLocalWorkerOwner,
};
use super::{LocalPreloadError, LocalTranscriptionError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreferredWhisperBackend {
    Vulkan { gpu_ordinal: i32 },
    Cpu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocalInferenceBackend {
    Vulkan,
    Cpu,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LocalContextSelection {
    pub backend: LocalInferenceBackend,
    pub vulkan_device: Option<String>,
    pub vulkan_device_kind: Option<VulkanDeviceKind>,
    pub cpu_fallback: bool,
}

impl LocalContextSelection {
    fn vulkan(device: &VulkanDeviceInfo) -> Self {
        Self {
            backend: LocalInferenceBackend::Vulkan,
            vulkan_device: Some(vulkan_device_label(device)),
            vulkan_device_kind: Some(device.kind),
            cpu_fallback: false,
        }
    }

    fn cpu_without_vulkan() -> Self {
        Self {
            backend: LocalInferenceBackend::Cpu,
            vulkan_device: None,
            vulkan_device_kind: None,
            cpu_fallback: false,
        }
    }

    fn cpu_fallback(device: &VulkanDeviceInfo) -> Self {
        Self {
            backend: LocalInferenceBackend::Cpu,
            vulkan_device: Some(vulkan_device_label(device)),
            vulkan_device_kind: Some(device.kind),
            cpu_fallback: true,
        }
    }
}

fn vulkan_device_label(device: &VulkanDeviceInfo) -> String {
    if device.description.trim().is_empty() {
        device.name.clone()
    } else {
        device.description.clone()
    }
}

fn log_context_selection(selection: &LocalContextSelection) {
    if std::env::var("PORTUS_ECHOES_NATIVE_LOG").as_deref() != Ok("1") {
        return;
    }
    let backend = match selection.backend {
        LocalInferenceBackend::Vulkan => "vulkan",
        LocalInferenceBackend::Cpu => "cpu",
    };
    let device_kind = match selection.vulkan_device_kind {
        Some(VulkanDeviceKind::Discrete) => "gpu",
        Some(VulkanDeviceKind::Integrated) => "igpu",
        None => "none",
    };
    eprintln!(
        "LOCAL_INFERENCE_BACKEND backend={backend} vulkan_device_type={device_kind} cpu_fallback={} device={:?}",
        selection.cpu_fallback,
        selection.vulkan_device.as_deref().unwrap_or("none")
    );
}

fn best_device_of_kind(
    devices: &[VulkanDeviceInfo],
    kind: VulkanDeviceKind,
) -> Option<&VulkanDeviceInfo> {
    let mut best = None;
    for device in devices.iter().filter(|device| device.kind == kind) {
        if best
            .map(|current: &VulkanDeviceInfo| device.memory_total > current.memory_total)
            .unwrap_or(true)
        {
            best = Some(device);
        }
    }
    best
}

pub(super) fn preferred_whisper_backend(devices: &[VulkanDeviceInfo]) -> PreferredWhisperBackend {
    best_device_of_kind(devices, VulkanDeviceKind::Discrete)
        .or_else(|| best_device_of_kind(devices, VulkanDeviceKind::Integrated))
        .map(|device| PreferredWhisperBackend::Vulkan {
            gpu_ordinal: device.whisper_gpu_ordinal,
        })
        .unwrap_or(PreferredWhisperBackend::Cpu)
}

pub(super) fn create_context_with_fallback<T, E>(
    model_path: &Path,
    devices: &[VulkanDeviceInfo],
    mut create: impl FnMut(&Path, bool, i32) -> Result<T, E>,
) -> Result<(T, LocalContextSelection), E> {
    match preferred_whisper_backend(devices) {
        PreferredWhisperBackend::Vulkan { gpu_ordinal } => {
            let selected_device = devices
                .iter()
                .find(|device| device.whisper_gpu_ordinal == gpu_ordinal)
                .expect("preferred Vulkan ordinal must come from the enumerated device set");
            match create(model_path, true, gpu_ordinal) {
                Ok(context) => Ok((context, LocalContextSelection::vulkan(selected_device))),
                Err(_) => create(model_path, false, 0).map(|context| {
                    (
                        context,
                        LocalContextSelection::cpu_fallback(selected_device),
                    )
                }),
            }
        }
        PreferredWhisperBackend::Cpu => create(model_path, false, 0)
            .map(|context| (context, LocalContextSelection::cpu_without_vulkan())),
    }
}

pub(super) fn create_whisper_context_with_selection(
    model_path: &Path,
) -> Result<(WhisperContext, LocalContextSelection), WhisperError> {
    super::super::configure_native_logging();

    #[cfg(test)]
    if std::env::var("PORTUS_ECHOES_TEST_LOCAL_BACKEND").as_deref() == Ok("cpu") {
        let mut context_params = WhisperContextParameters::default();
        context_params.use_gpu(false);
        context_params.gpu_device(0);
        context_params.flash_attn(false);
        let context = WhisperContext::new_with_params(model_path, context_params)?;
        let selection = LocalContextSelection::cpu_without_vulkan();
        log_context_selection(&selection);
        return Ok((context, selection));
    }

    let devices = list_vulkan_devices();
    let (context, selection) =
        create_context_with_fallback(model_path, &devices, |path, use_gpu, gpu_ordinal| {
            if use_gpu {
                let mut flash_params = WhisperContextParameters::default();
                flash_params.use_gpu(true);
                flash_params.gpu_device(gpu_ordinal);
                flash_params.flash_attn(true);
                if let Ok(ctx) = WhisperContext::new_with_params(path, flash_params) {
                    return Ok(ctx);
                }
            }
            let mut context_params = WhisperContextParameters::default();
            context_params.use_gpu(use_gpu);
            context_params.gpu_device(gpu_ordinal);
            context_params.flash_attn(false);
            WhisperContext::new_with_params(path, context_params)
        })?;
    log_context_selection(&selection);
    Ok((context, selection))
}

#[derive(Clone)]
pub(crate) struct ReadyLocalModel {
    path: PathBuf,
    inference: SharedLocalInferenceClient,
}

impl ReadyLocalModel {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn is_alive(&self) -> bool {
        self.inference.is_alive()
    }


    #[cfg(test)]
    pub(super) fn test_with_inference(
        path: PathBuf,
        inference: SharedLocalInferenceClient,
    ) -> Self {
        Self { path, inference }
    }

    #[allow(dead_code)]
    pub(super) fn transcribe(
        &self,
        audio: &[f32],
        language: Option<&str>,
        abort_requested: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Option<String>, LocalTranscriptionError> {
        self.inference.transcribe(audio, language, abort_requested)
    }

    pub(super) fn transcribe_stream(
        &self,
        audio: &[f32],
        language: Option<&str>,
        abort_requested: Arc<std::sync::atomic::AtomicBool>,
        on_segment: &mut dyn FnMut(String) -> Result<(), LocalTranscriptionError>,
    ) -> Result<Option<String>, LocalTranscriptionError> {
        self.inference
            .transcribe_stream(audio, language, abort_requested, on_segment)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalRuntimeTarget {
    Unloaded,
    Load(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalRuntimeIntent {
    generation: u64,
    target: LocalRuntimeTarget,
}

impl LocalRuntimeIntent {
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub(crate) fn target(&self) -> &LocalRuntimeTarget {
        &self.target
    }
}

pub(crate) struct LocalRuntimeAuthority {
    current: Mutex<LocalRuntimeIntent>,
}

impl LocalRuntimeAuthority {
    pub(crate) fn new(target: LocalRuntimeTarget) -> Self {
        Self {
            current: Mutex::new(LocalRuntimeIntent {
                generation: 1,
                target,
            }),
        }
    }

    pub(crate) fn publish(&self, target: LocalRuntimeTarget) -> LocalRuntimeIntent {
        let mut current = self.current.lock();
        let generation = current
            .generation
            .checked_add(1)
            .expect("Local runtime generation exhausted");
        let intent = LocalRuntimeIntent { generation, target };
        *current = intent.clone();
        intent
    }

    pub(crate) fn current(&self) -> LocalRuntimeIntent {
        self.current.lock().clone()
    }

    pub(crate) fn current_target(&self) -> LocalRuntimeTarget {
        self.current.lock().target.clone()
    }

    fn run_if_current<R>(
        &self,
        intent: &LocalRuntimeIntent,
        action: impl FnOnce() -> R,
    ) -> Option<R> {
        let current = self.current.lock();
        if *current != *intent {
            return None;
        }
        Some(action())
    }

    fn inspect_current<R>(&self, inspect: impl FnOnce(&LocalRuntimeIntent) -> R) -> R {
        let current = self.current.lock();
        inspect(&current)
    }
}

#[cfg_attr(not(test), allow(dead_code))]
enum LocalModelState {
    Unloaded,
    Loading {
        generation: u64,
        path: PathBuf,
        owner: Option<SharedLocalWorkerOwner>,
    },
    Ready {
        generation: u64,
        path: PathBuf,
        model: ReadyLocalModel,
    },
    Failed {
        generation: u64,
        path: PathBuf,
    },
}

struct AppliedLocalModelState {
    generation: u64,
    model: LocalModelState,
}

struct LocalEngineInner {
    authority: Arc<LocalRuntimeAuthority>,
    state: Mutex<AppliedLocalModelState>,
    shutdown: AtomicBool,
    spawn_worker: LocalWorkerSpawner,
}

/// Application-owned Local model supervisor.
///
/// Model loading belongs to app/model selection, never recording preparation.
/// Loading workers are separate same-executable child processes so selecting a
/// different model/provider can terminate and reap a native initialization that
/// has not returned. Ready workers remain alive while an already-started
/// recording retains its `ReadyLocalModel` clone.
#[derive(Clone)]
pub struct LocalEngine {
    inner: Arc<LocalEngineInner>,
}

impl LocalEngine {
    pub(crate) fn with_authority(authority: Arc<LocalRuntimeAuthority>) -> Self {
        Self::with_authority_and_spawner(authority, Arc::new(spawn_local_worker))
    }

    fn with_authority_and_spawner(
        authority: Arc<LocalRuntimeAuthority>,
        spawn_worker: LocalWorkerSpawner,
    ) -> Self {
        Self {
            inner: Arc::new(LocalEngineInner {
                authority,
                state: Mutex::new(AppliedLocalModelState {
                    generation: 0,
                    model: LocalModelState::Unloaded,
                }),
                shutdown: AtomicBool::new(false),
                spawn_worker,
            }),
        }
    }

    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_authority(Arc::new(LocalRuntimeAuthority::new(
            LocalRuntimeTarget::Unloaded,
        )))
    }

    #[cfg(test)]
    pub(crate) fn with_controlled_workers(
        authority: Arc<LocalRuntimeAuthority>,
    ) -> (
        Self,
        std::sync::mpsc::Receiver<super::worker::ControlledWorkerProbe>,
    ) {
        let (spawner, spawned) = super::worker::controlled_worker_spawner();
        (
            Self::with_authority_and_spawner(authority, spawner),
            spawned,
        )
    }

    #[cfg(test)]
    pub(crate) fn with_in_process_workers(authority: Arc<LocalRuntimeAuthority>) -> Self {
        Self::with_authority_and_spawner(authority, super::worker::in_process_worker_spawner(None))
    }

    #[cfg(test)]
    pub(crate) fn with_in_process_worker_timeout(
        authority: Arc<LocalRuntimeAuthority>,
        timeout: Arc<Mutex<std::time::Duration>>,
    ) -> Self {
        Self::with_authority_and_spawner(
            authority,
            super::worker::in_process_worker_spawner(Some(timeout)),
        )
    }

    pub(crate) fn apply_intent(
        &self,
        intent: LocalRuntimeIntent,
        on_complete: impl Fn(PathBuf, Result<(), LocalPreloadError>) + Send + Sync + 'static,
    ) {
        let target = intent.target.clone();
        let previous = self
            .inner
            .authority
            .run_if_current(&intent, || {
                if self.inner.shutdown.load(Ordering::SeqCst) {
                    return None;
                }
                let mut state = self.inner.state.lock();
                if intent.generation <= state.generation {
                    return None;
                }
                state.generation = intent.generation;
                let next = match &target {
                    LocalRuntimeTarget::Unloaded => LocalModelState::Unloaded,
                    LocalRuntimeTarget::Load(path) => LocalModelState::Loading {
                        generation: intent.generation,
                        path: path.clone(),
                        owner: None,
                    },
                };
                Some(std::mem::replace(&mut state.model, next))
            })
            .flatten();
        let Some(previous) = previous else {
            return;
        };
        revoke_state(previous);

        let LocalRuntimeTarget::Load(model_path) = target else {
            return;
        };
        let on_complete = Arc::new(on_complete);

        if !model_path.is_file() {
            self.finish_load(
                intent,
                model_path,
                Err(LocalPreloadError::ModelNotFound),
                on_complete,
            );
            return;
        }

        let loading = self
            .inner
            .authority
            .run_if_current(&intent, || {
                if self.inner.shutdown.load(Ordering::SeqCst) {
                    return None;
                }
                let mut state = self.inner.state.lock();
                match &mut state.model {
                    LocalModelState::Loading {
                        generation,
                        path,
                        owner,
                    } if *generation == intent.generation && *path == model_path => {
                        let loading = (self.inner.spawn_worker)(&model_path);
                        if let Ok(worker) = &loading {
                            *owner = Some(worker.owner());
                        }
                        Some(loading)
                    }
                    _ => None,
                }
            })
            .flatten();
        let Some(loading) = loading else {
            return;
        };
        let loading = match loading {
            Ok(loading) => loading,
            Err(error) => {
                self.finish_load(intent, model_path, Err(error), on_complete);
                return;
            }
        };

        // The worker owner is installed under the same model-state lock that
        // protected spawning. Shutdown/replacement therefore cannot observe a
        // spawned Loading child without also owning the handle needed to reap it.
        let still_current = self
            .inner
            .authority
            .run_if_current(&intent, || {
                let state = self.inner.state.lock();
                matches!(
                    &state.model,
                    LocalModelState::Loading {
                        generation,
                        path,
                        owner: Some(_),
                    } if *generation == intent.generation && *path == model_path
                )
            })
            .unwrap_or(false);
        if !still_current {
            loading.cancel();
            return;
        }

        let engine = self.clone();
        std::thread::spawn(move || {
            let result = loading.wait_ready();
            engine.finish_load(intent, model_path, result, on_complete);
        });
    }

    fn finish_load(
        &self,
        intent: LocalRuntimeIntent,
        model_path: PathBuf,
        result: Result<SharedLocalInferenceClient, LocalPreloadError>,
        on_complete: Arc<dyn Fn(PathBuf, Result<(), LocalPreloadError>) + Send + Sync>,
    ) {
        let _ = self.inner.authority.run_if_current(&intent, || {
            if self.inner.shutdown.load(Ordering::SeqCst) {
                return;
            }
            let mut state = self.inner.state.lock();
            let is_current = state.generation == intent.generation
                && matches!(
                    &state.model,
                    LocalModelState::Loading {
                        generation,
                        path,
                        ..
                    } if *generation == intent.generation && *path == model_path
                );
            if !is_current {
                return;
            }

            let completion = match result {
                Ok(inference) => {
                    let model = ReadyLocalModel {
                        path: model_path.clone(),
                        inference,
                    };
                    state.model = LocalModelState::Ready {
                        generation: intent.generation,
                        path: model_path.clone(),
                        model,
                    };
                    Ok(())
                }
                Err(error) => {
                    state.model = LocalModelState::Failed {
                        generation: intent.generation,
                        path: model_path.clone(),
                    };
                    Err(error)
                }
            };
            drop(state);
            on_complete(model_path, completion);
        });
    }

    pub(crate) fn shutdown(&self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
        let previous = {
            let mut state = self.inner.state.lock();
            std::mem::replace(&mut state.model, LocalModelState::Unloaded)
        };
        revoke_state(previous);
    }

    /// Clone only an already-Ready model matching the exact authoritative
    /// selected path and generation. This method never loads or reloads a model.
    pub(crate) fn ready_model(&self, model_path: &Path) -> Option<ReadyLocalModel> {
        if self.inner.shutdown.load(Ordering::SeqCst) {
            return None;
        }
        let intent = self.inner.authority.current();
        let LocalRuntimeTarget::Load(authoritative_path) = &intent.target else {
            return None;
        };
        if authoritative_path != model_path {
            return None;
        }

        let mut needs_reload = false;
        let ready = {
            let mut state = self.inner.state.lock();
            if state.generation != intent.generation {
                return None;
            }
            match &state.model {
                LocalModelState::Ready {
                    generation,
                    path,
                    model,
                } if *generation == intent.generation && path == model_path => {
                    if !model.is_alive() {
                        let dead = std::mem::replace(&mut state.model, LocalModelState::Unloaded);
                        drop(state);
                        revoke_state(dead);
                        needs_reload = true;
                        None
                    } else {
                        Some(model.clone())
                    }
                }
                LocalModelState::Unloaded | LocalModelState::Failed { .. } => {
                    drop(state);
                    needs_reload = true;
                    None
                }
                _ => None,
            }
        };

        if needs_reload {
            let reload_intent = self
                .inner
                .authority
                .publish(LocalRuntimeTarget::Load(authoritative_path.clone()));
            self.apply_intent(reload_intent, |_, _| {});
            return None;
        }

        ready
    }

    /// Immediately recover the authoritative model if it has died or failed,
    /// so the replacement worker is loaded and ready before the user's next keypress.
    pub(crate) fn recover_active_model(&self) {
        if self.inner.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let intent = self.inner.authority.current();
        let LocalRuntimeTarget::Load(authoritative_path) = &intent.target else {
            return;
        };

        let mut needs_reload = false;
        {
            let mut state = self.inner.state.lock();
            if state.generation == intent.generation {
                match &state.model {
                    LocalModelState::Ready { model, .. } => {
                        if !model.is_alive() {
                            let dead =
                                std::mem::replace(&mut state.model, LocalModelState::Unloaded);
                            drop(state);
                            revoke_state(dead);
                            needs_reload = true;
                        }
                    }
                    LocalModelState::Unloaded | LocalModelState::Failed { .. } => {
                        needs_reload = true;
                    }
                    _ => {}
                }
            } else {
                needs_reload = true;
            }
        }

        if needs_reload {
            let reload_intent = self
                .inner
                .authority
                .publish(LocalRuntimeTarget::Load(authoritative_path.clone()));
            self.apply_intent(reload_intent, |_, _| {});
        }
    }
    #[cfg(test)]
    pub(crate) fn is_ready_for(&self, model_path: &Path) -> bool {
        self.ready_model(model_path).is_some()
    }

    /// Returns the exact model path currently Ready for recording, if any.
    #[allow(dead_code)]
    pub fn cached_path(&self) -> Option<PathBuf> {
        if self.inner.shutdown.load(Ordering::SeqCst) {
            return None;
        }
        self.inner.authority.inspect_current(|intent| {
            let LocalRuntimeTarget::Load(authoritative_path) = &intent.target else {
                return None;
            };
            let state = self.inner.state.lock();
            if state.generation != intent.generation {
                return None;
            }
            match &state.model {
                LocalModelState::Ready {
                    generation, path, ..
                } if *generation == intent.generation && path == authoritative_path => {
                    Some(path.clone())
                }
                _ => None,
            }
        })
    }


    #[cfg(test)]
    pub(crate) fn state_generation_and_path(&self) -> (u64, Option<PathBuf>, &'static str) {
        let state = self.inner.state.lock();
        match &state.model {
            LocalModelState::Unloaded => (state.generation, None, "unloaded"),
            LocalModelState::Loading {
                generation, path, ..
            } => (*generation, Some(path.clone()), "loading"),
            LocalModelState::Ready {
                generation, path, ..
            } => (*generation, Some(path.clone()), "ready"),
            LocalModelState::Failed { generation, path } => {
                (*generation, Some(path.clone()), "failed")
            }
        }
    }
}

fn revoke_state(state: LocalModelState) {
    if let LocalModelState::Loading {
        owner: Some(owner), ..
    } = state
    {
        owner.terminate_and_reap();
    }
}

#[cfg(test)]
mod readiness_tests {
    use super::super::worker::WorkerProcessOwner;
    use super::*;

    struct NoopInference;

    impl super::super::worker::LocalInferenceClient for NoopInference {
        fn transcribe(
            &self,
            _audio: &[f32],
            _language: Option<&str>,
            _abort_requested: Arc<std::sync::atomic::AtomicBool>,
        ) -> Result<Option<String>, LocalTranscriptionError> {
            Ok(None)
        }
    }

    struct DeadInference;

    impl super::super::worker::LocalInferenceClient for DeadInference {
        fn is_alive(&self) -> bool {
            false
        }
        fn transcribe(
            &self,
            _audio: &[f32],
            _language: Option<&str>,
            _abort_requested: Arc<std::sync::atomic::AtomicBool>,
        ) -> Result<Option<String>, LocalTranscriptionError> {
            Err(LocalTranscriptionError::Inference)
        }
    }


    fn engine_with_target(
        target: LocalRuntimeTarget,
    ) -> (Arc<LocalRuntimeAuthority>, LocalEngine, LocalRuntimeIntent) {
        let authority = Arc::new(LocalRuntimeAuthority::new(target));
        let intent = authority.current();
        let engine = LocalEngine::with_authority(Arc::clone(&authority));
        (authority, engine, intent)
    }

    fn model_file(dir: &tempfile::TempDir, name: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, b"controlled worker model").unwrap();
        path
    }

    fn ready_model(path: &Path) -> ReadyLocalModel {
        ReadyLocalModel {
            path: path.to_path_buf(),
            inference: Arc::new(NoopInference),
        }
    }

    fn install_ready(engine: &LocalEngine, intent: &LocalRuntimeIntent, path: &Path) {
        *engine.inner.state.lock() = AppliedLocalModelState {
            generation: intent.generation,
            model: LocalModelState::Ready {
                generation: intent.generation,
                path: path.to_path_buf(),
                model: ready_model(path),
            },
        };
    }

    #[test]
    fn ready_lookup_requires_authoritative_generation_and_exact_path() {
        let (authority, engine, intent) =
            engine_with_target(LocalRuntimeTarget::Load(PathBuf::from("a.bin")));
        install_ready(&engine, &intent, Path::new("a.bin"));

        assert!(engine.is_ready_for(Path::new("a.bin")));
        assert!(!engine.is_ready_for(Path::new("b.bin")));

        authority.publish(LocalRuntimeTarget::Load(PathBuf::from("b.bin")));
        assert!(!engine.is_ready_for(Path::new("a.bin")));
        assert_eq!(engine.cached_path(), None);
    }

    #[test]
    fn ready_model_detects_dead_worker_and_triggers_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = model_file(&dir, "model.bin");
        let authority = Arc::new(LocalRuntimeAuthority::new(LocalRuntimeTarget::Load(
            path.clone(),
        )));
        let intent = authority.current();
        let (engine, spawned) = LocalEngine::with_controlled_workers(Arc::clone(&authority));
        *engine.inner.state.lock() = AppliedLocalModelState {
            generation: intent.generation,
            model: LocalModelState::Ready {
                generation: intent.generation,
                path: path.clone(),
                model: ReadyLocalModel {
                    path: path.clone(),
                    inference: Arc::new(DeadInference),
                },
            },
        };

        let result = engine.ready_model(&path);
        assert!(result.is_none());

        let worker = spawned
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(worker.path(), path);

        let new_intent = authority.current();
        assert!(new_intent.generation > intent.generation);

        worker.complete_ready();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !engine.is_ready_for(&path) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(engine.is_ready_for(&path));
    }

    #[test]
    fn recover_active_model_immediately_respawns_dead_worker() {
        let dir = tempfile::tempdir().unwrap();
        let path = model_file(&dir, "model.bin");
        let authority = Arc::new(LocalRuntimeAuthority::new(LocalRuntimeTarget::Load(
            path.clone(),
        )));
        let intent = authority.current();
        let (engine, spawned) = LocalEngine::with_controlled_workers(Arc::clone(&authority));
        *engine.inner.state.lock() = AppliedLocalModelState {
            generation: intent.generation,
            model: LocalModelState::Ready {
                generation: intent.generation,
                path: path.clone(),
                model: ReadyLocalModel {
                    path: path.clone(),
                    inference: Arc::new(DeadInference),
                },
            },
        };

        engine.recover_active_model();

        let worker = spawned
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(worker.path(), path);

        let new_intent = authority.current();
        assert!(new_intent.generation > intent.generation);

        worker.complete_ready();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !engine.is_ready_for(&path) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(engine.is_ready_for(&path));
    }

    #[test]
    fn authority_publication_blocks_old_completion_before_new_intent_is_applied() {
        let (authority, engine, old_intent) =
            engine_with_target(LocalRuntimeTarget::Load(PathBuf::from("a.bin")));
        *engine.inner.state.lock() = AppliedLocalModelState {
            generation: old_intent.generation,
            model: LocalModelState::Loading {
                generation: old_intent.generation,
                path: PathBuf::from("a.bin"),
                owner: None,
            },
        };
        authority.publish(LocalRuntimeTarget::Load(PathBuf::from("b.bin")));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&calls);

        engine.finish_load(
            old_intent,
            PathBuf::from("a.bin"),
            Ok(Arc::new(NoopInference)),
            Arc::new(move |path, result| observed.lock().push((path, result))),
        );

        assert_eq!(
            engine.state_generation_and_path(),
            (1, Some(PathBuf::from("a.bin")), "loading")
        );
        assert!(calls.lock().is_empty());
        assert_eq!(engine.cached_path(), None);
    }

    #[test]
    fn delayed_model_intent_cannot_replace_newer_selection() {
        let (authority, engine, model_a) =
            engine_with_target(LocalRuntimeTarget::Load(PathBuf::from("missing-a.bin")));
        let model_b = authority.publish(LocalRuntimeTarget::Load(PathBuf::from("missing-b.bin")));
        let calls = Arc::new(Mutex::new(Vec::new()));

        let observed_b = Arc::clone(&calls);
        engine.apply_intent(model_b.clone(), move |path, result| {
            observed_b.lock().push((path, result))
        });
        let observed_a = Arc::clone(&calls);
        engine.apply_intent(model_a, move |path, result| {
            observed_a.lock().push((path, result))
        });

        assert_eq!(
            engine.state_generation_and_path(),
            (
                model_b.generation,
                Some(PathBuf::from("missing-b.bin")),
                "failed"
            )
        );
        assert_eq!(calls.lock().len(), 1);
        assert_eq!(calls.lock()[0].0, PathBuf::from("missing-b.bin"));
    }

    #[test]
    fn delayed_local_intent_cannot_repopulate_after_cloud_unload() {
        let (authority, engine, local) =
            engine_with_target(LocalRuntimeTarget::Load(PathBuf::from("missing-a.bin")));
        let cloud = authority.publish(LocalRuntimeTarget::Unloaded);

        engine.apply_intent(cloud.clone(), |_, _| {});
        engine.apply_intent(local, |_, _| panic!("stale Local intent must be ignored"));

        assert_eq!(
            engine.state_generation_and_path(),
            (cloud.generation, None, "unloaded")
        );
        assert_eq!(engine.cached_path(), None);
    }

    #[test]
    fn same_model_reselection_uses_new_generation_and_retries_load() {
        let (authority, engine, first) =
            engine_with_target(LocalRuntimeTarget::Load(PathBuf::from("missing.bin")));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let first_calls = Arc::clone(&calls);
        engine.apply_intent(first, move |path, result| {
            first_calls.lock().push((path, result))
        });
        let retry = authority.publish(LocalRuntimeTarget::Load(PathBuf::from("missing.bin")));
        let retry_calls = Arc::clone(&calls);
        engine.apply_intent(retry.clone(), move |path, result| {
            retry_calls.lock().push((path, result))
        });

        assert_eq!(
            engine.state_generation_and_path(),
            (
                retry.generation,
                Some(PathBuf::from("missing.bin")),
                "failed"
            )
        );
        assert_eq!(calls.lock().len(), 2);
    }

    #[test]
    fn model_replacement_terminates_and_reaps_loading_process_promptly() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        #[cfg(target_os = "windows")]
        let child = Command::new("powershell")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("test sleep child must start");

        #[cfg(not(target_os = "windows"))]
        let child = Command::new("sh")
            .args(["-c", "sleep 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("test sleep child must start");

        let (authority, engine, loading) =
            engine_with_target(LocalRuntimeTarget::Load(PathBuf::from("a.bin")));
        *engine.inner.state.lock() = AppliedLocalModelState {
            generation: loading.generation,
            model: LocalModelState::Loading {
                generation: loading.generation,
                path: PathBuf::from("a.bin"),
                owner: Some(Arc::new(WorkerProcessOwner::new(child))),
            },
        };
        let replacement =
            authority.publish(LocalRuntimeTarget::Load(PathBuf::from("missing-b.bin")));

        let started = Instant::now();
        engine.apply_intent(replacement.clone(), |_, _| {});

        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            engine.state_generation_and_path(),
            (
                replacement.generation,
                Some(PathBuf::from("missing-b.bin")),
                "failed"
            )
        );
    }

    #[test]
    fn unload_revokes_ready_eligibility_but_retained_recording_handle_survives() {
        let (authority, engine, ready) =
            engine_with_target(LocalRuntimeTarget::Load(PathBuf::from("a.bin")));
        install_ready(&engine, &ready, Path::new("a.bin"));
        let retained = engine.ready_model(Path::new("a.bin")).unwrap();
        let unload = authority.publish(LocalRuntimeTarget::Unloaded);

        engine.apply_intent(unload.clone(), |_, _| {});

        assert!(!engine.is_ready_for(Path::new("a.bin")));
        assert_eq!(retained.path(), Path::new("a.bin"));
        assert_eq!(
            engine.state_generation_and_path(),
            (unload.generation, None, "unloaded")
        );
    }

    #[test]
    fn shutdown_is_idempotent_and_permanently_revokes_readiness() {
        let (_authority, engine, ready) =
            engine_with_target(LocalRuntimeTarget::Load(PathBuf::from("a.bin")));
        install_ready(&engine, &ready, Path::new("a.bin"));
        let retained = engine.ready_model(Path::new("a.bin")).unwrap();

        engine.shutdown();
        engine.shutdown();

        assert!(!engine.is_ready_for(Path::new("a.bin")));
        assert_eq!(engine.cached_path(), None);
        assert_eq!(retained.path(), Path::new("a.bin"));
        assert_eq!(
            engine.state_generation_and_path(),
            (ready.generation, None, "unloaded")
        );
    }

    #[test]
    fn apply_intent_starts_worker_and_publishes_exact_ready_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = model_file(&dir, "startup.bin");
        let authority = Arc::new(LocalRuntimeAuthority::new(LocalRuntimeTarget::Load(
            path.clone(),
        )));
        let intent = authority.current();
        let (engine, spawned) = LocalEngine::with_controlled_workers(authority);
        let (completed_tx, completed_rx) = std::sync::mpsc::channel();

        engine.apply_intent(intent, move |path, result| {
            completed_tx.send((path, result)).unwrap();
        });
        let worker = spawned
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(worker.path(), path);
        assert!(!engine.is_ready_for(&path));

        worker.complete_ready();
        assert_eq!(
            completed_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            (path.clone(), Ok(()))
        );
        assert!(engine.is_ready_for(&path));
    }

    #[test]
    fn spawned_loading_child_stays_under_state_lock_until_reap_owner_is_registered() {
        use std::sync::Barrier;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let path = model_file(&dir, "spawn-race.bin");
        let authority = Arc::new(LocalRuntimeAuthority::new(LocalRuntimeTarget::Load(
            path.clone(),
        )));
        let intent = authority.current();
        let (base_spawner, spawned) = super::super::worker::controlled_worker_spawner();
        let child_spawned = Arc::new(Barrier::new(2));
        let release_spawner = Arc::new(Barrier::new(2));
        let spawner_child_spawned = Arc::clone(&child_spawned);
        let spawner_release = Arc::clone(&release_spawner);
        let blocking_spawner: LocalWorkerSpawner = Arc::new(move |model_path| {
            let worker = base_spawner(model_path)?;
            spawner_child_spawned.wait();
            spawner_release.wait();
            Ok(worker)
        });
        let engine = LocalEngine::with_authority_and_spawner(authority, blocking_spawner);
        let apply_engine = engine.clone();

        let apply = std::thread::spawn(move || {
            apply_engine.apply_intent(intent, |_, _| {});
        });

        child_spawned.wait();
        let worker = spawned.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            engine.inner.state.try_lock().is_none(),
            "model-state lock must still cover the successfully spawned child until its reap owner is registered"
        );

        release_spawner.wait();
        apply.join().unwrap();

        {
            let state = engine.inner.state.lock();
            assert!(matches!(
                &state.model,
                LocalModelState::Loading {
                    path: loading_path,
                    owner: Some(_),
                    ..
                } if loading_path == &path
            ));
        }

        engine.shutdown();
        assert!(worker.is_terminated());
        assert!(worker.wait_until_reaped(Duration::from_secs(1)));
    }

    #[test]
    fn model_replacement_reaps_loading_a_and_suppresses_its_completion() {
        let dir = tempfile::tempdir().unwrap();
        let path_a = model_file(&dir, "a.bin");
        let path_b = model_file(&dir, "b.bin");
        let authority = Arc::new(LocalRuntimeAuthority::new(LocalRuntimeTarget::Load(
            path_a.clone(),
        )));
        let intent_a = authority.current();
        let (engine, spawned) = LocalEngine::with_controlled_workers(Arc::clone(&authority));
        let (completion_tx, completion_rx) = std::sync::mpsc::channel();
        let completion_a = completion_tx.clone();
        engine.apply_intent(intent_a, move |path, result| {
            completion_a.send((path, result)).unwrap();
        });
        let worker_a = spawned
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();

        let intent_b = authority.publish(LocalRuntimeTarget::Load(path_b.clone()));
        engine.apply_intent(intent_b, move |path, result| {
            completion_tx.send((path, result)).unwrap();
        });
        let worker_b = spawned
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();

        assert!(worker_a.is_terminated());
        assert!(worker_a.wait_until_reaped(std::time::Duration::from_secs(1)));
        worker_b.complete_ready();
        assert_eq!(
            completion_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            (path_b.clone(), Ok(()))
        );
        assert!(completion_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err());
        assert!(!engine.is_ready_for(&path_a));
        assert!(engine.is_ready_for(&path_b));
    }

    #[test]
    fn failed_model_reselection_spawns_a_new_generation_and_reaches_ready() {
        let dir = tempfile::tempdir().unwrap();
        let path = model_file(&dir, "retry.bin");
        let authority = Arc::new(LocalRuntimeAuthority::new(LocalRuntimeTarget::Load(
            path.clone(),
        )));
        let first = authority.current();
        let (engine, spawned) = LocalEngine::with_controlled_workers(Arc::clone(&authority));
        let (completed_tx, completed_rx) = std::sync::mpsc::channel();
        let first_completed = completed_tx.clone();
        engine.apply_intent(first, move |path, result| {
            first_completed.send((path, result)).unwrap();
        });
        let failed_worker = spawned
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        failed_worker.complete_error(LocalPreloadError::UnsupportedModel);
        assert_eq!(
            completed_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            (path.clone(), Err(LocalPreloadError::UnsupportedModel))
        );
        assert_eq!(engine.state_generation_and_path().2, "failed");

        let retry = authority.publish(LocalRuntimeTarget::Load(path.clone()));
        engine.apply_intent(retry, move |path, result| {
            completed_tx.send((path, result)).unwrap();
        });
        let retry_worker = spawned
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        retry_worker.complete_ready();
        assert_eq!(
            completed_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            (path.clone(), Ok(()))
        );
        assert!(engine.is_ready_for(&path));
    }

    #[test]
    fn retired_ready_worker_remains_usable_until_recording_handle_drops() {
        let dir = tempfile::tempdir().unwrap();
        let path = model_file(&dir, "retained.bin");
        let authority = Arc::new(LocalRuntimeAuthority::new(LocalRuntimeTarget::Load(
            path.clone(),
        )));
        let ready = authority.current();
        let (engine, spawned) = LocalEngine::with_controlled_workers(Arc::clone(&authority));
        let (completed_tx, completed_rx) = std::sync::mpsc::channel();
        engine.apply_intent(ready, move |_, result| {
            completed_tx.send(result).unwrap();
        });
        let worker = spawned
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        worker.complete_ready();
        assert_eq!(
            completed_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            Ok(())
        );
        let retained = engine.ready_model(&path).unwrap();

        let unload = authority.publish(LocalRuntimeTarget::Unloaded);
        engine.apply_intent(unload, |_, _| {});

        assert!(!engine.is_ready_for(&path));
        assert!(!worker.wait_until_reaped(std::time::Duration::from_millis(50)));
        assert_eq!(
            retained.transcribe(
                &[0.25],
                Some("en"),
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            ),
            Ok(None)
        );
        drop(retained);
        assert!(worker.wait_until_reaped(std::time::Duration::from_secs(1)));
    }

    #[test]
    fn shutdown_terminates_and_reaps_loading_process_promptly() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        #[cfg(target_os = "windows")]
        let child = Command::new("powershell")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("test sleep child must start");

        #[cfg(not(target_os = "windows"))]
        let child = Command::new("sh")
            .args(["-c", "sleep 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("test sleep child must start");

        let (_authority, engine, loading) =
            engine_with_target(LocalRuntimeTarget::Load(PathBuf::from("a.bin")));
        *engine.inner.state.lock() = AppliedLocalModelState {
            generation: loading.generation,
            model: LocalModelState::Loading {
                generation: loading.generation,
                path: PathBuf::from("a.bin"),
                owner: Some(Arc::new(WorkerProcessOwner::new(child))),
            },
        };

        let started = Instant::now();
        engine.shutdown();

        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            engine.state_generation_and_path(),
            (loading.generation, None, "unloaded")
        );
    }
}
