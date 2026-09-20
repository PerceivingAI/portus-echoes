use std::ffi::OsString;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::Mutex;
use whisper_rs::{WhisperContext, WhisperError};

use super::engine::create_whisper_context_with_selection;
use super::inference::LOCAL_INFERENCE_TIMEOUT;
use super::{LocalPreloadError, LocalTranscriptionError};

const INTERNAL_LOCAL_WORKER_FLAG: &str = "--internal-local-worker";
const COMMAND_INFER: u8 = 1;
const COMMAND_ABORT: u8 = 2;
const RESPONSE_READY: u8 = 10;
const RESPONSE_LOAD_ERROR: u8 = 11;
const RESPONSE_INFERENCE: u8 = 12;
const INFERENCE_NONE: u8 = 0;
const INFERENCE_TEXT: u8 = 1;
const INFERENCE_ERROR: u8 = 2;
const INFERENCE_CHUNK: u8 = 3;
const INFERENCE_DONE: u8 = 4;
const ABORT_POLL_INTERVAL: Duration = Duration::from_millis(5);
const WORKER_COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(5);
const MAX_WORKER_SAMPLES: usize = 512_000;
const MAX_WORKER_LANGUAGE_BYTES: usize = 64;
const MAX_WORKER_TEXT_BYTES: usize = 16 * 1024 * 1024;

pub(super) trait LocalInferenceClient: Send + Sync {
    fn is_alive(&self) -> bool {
        true
    }

    fn transcribe(
        &self,
        audio: &[f32],
        language: Option<&str>,
        abort_requested: Arc<AtomicBool>,
    ) -> Result<Option<String>, LocalTranscriptionError>;

    fn transcribe_stream(
        &self,
        audio: &[f32],
        language: Option<&str>,
        abort_requested: Arc<AtomicBool>,
        on_segment: &mut dyn FnMut(String) -> Result<(), LocalTranscriptionError>,
    ) -> Result<Option<String>, LocalTranscriptionError> {
        let result = self.transcribe(audio, language, abort_requested)?;
        if let Some(text) = result.clone() {
            on_segment(text)?;
        }
        Ok(result)
    }
}

pub(super) type SharedLocalInferenceClient = Arc<dyn LocalInferenceClient>;
type WorkerWriter = Box<dyn Write + Send>;
type WorkerReader = Box<dyn Read + Send>;

pub(super) trait LocalWorkerOwner: Send + Sync {
    fn terminate_and_reap(&self);
    fn is_alive(&self) -> bool {
        true
    }
}

pub(super) type SharedLocalWorkerOwner = Arc<dyn LocalWorkerOwner>;
pub(super) type LocalWorkerSpawner =
    Arc<dyn Fn(&Path) -> Result<LoadingLocalWorker, LocalPreloadError> + Send + Sync + 'static>;

pub(super) struct WorkerProcessOwner {
    child: Mutex<Option<Child>>,
}

impl WorkerProcessOwner {
    pub(super) fn new(child: Child) -> Self {
        Self {
            child: Mutex::new(Some(child)),
        }
    }
    pub(super) fn is_alive(&self) -> bool {
        let mut guard = self.child.lock();
        if let Some(child) = guard.as_mut() {
            match child.try_wait() {
                Ok(None) => true,
                _ => false,
            }
        } else {
            false
        }
    }


    pub(super) fn terminate_and_reap(&self) {
        let Some(mut child) = self.child.lock().take() else {
            return;
        };
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl LocalWorkerOwner for WorkerProcessOwner {
    fn terminate_and_reap(&self) {
        WorkerProcessOwner::terminate_and_reap(self);
    }

    fn is_alive(&self) -> bool {
        WorkerProcessOwner::is_alive(self)
    }
}

impl Drop for WorkerProcessOwner {
    fn drop(&mut self) {
        let Some(mut child) = self.child.get_mut().take() else {
            return;
        };
        let _ = child.kill();
        let _ = child.wait();
    }
}

pub(super) struct LoadingLocalWorker {
    owner: SharedLocalWorkerOwner,
    writer: Option<WorkerWriter>,
    reader: Option<WorkerReader>,
}

impl LoadingLocalWorker {
    pub(super) fn owner(&self) -> SharedLocalWorkerOwner {
        Arc::clone(&self.owner)
    }

    pub(super) fn cancel(&self) {
        self.owner.terminate_and_reap();
    }

    pub(super) fn wait_ready(mut self) -> Result<SharedLocalInferenceClient, LocalPreloadError> {
        let reader = self.reader.as_mut().ok_or(LocalPreloadError::Unexpected)?;
        let tag = read_u8(reader.as_mut()).map_err(|_| LocalPreloadError::Unexpected)?;
        match tag {
            RESPONSE_READY => {
                let writer = self.writer.take().ok_or(LocalPreloadError::Unexpected)?;
                let reader = self.reader.take().ok_or(LocalPreloadError::Unexpected)?;
                Ok(Arc::new(LocalWorkerClient {
                    owner: Arc::clone(&self.owner),
                    writer: Arc::new(Mutex::new(writer)),
                    reader: Mutex::new(reader),
                    inference_gate: Mutex::new(()),
                    next_request_id: AtomicU64::new(1),
                }))
            }
            RESPONSE_LOAD_ERROR => {
                let code = read_u8(reader).map_err(|_| LocalPreloadError::Unexpected)?;
                Err(decode_preload_error(code).unwrap_or(LocalPreloadError::Unexpected))
            }
            _ => Err(LocalPreloadError::Unexpected),
        }
    }
}

pub(super) fn spawn_local_worker(
    model_path: &Path,
) -> Result<LoadingLocalWorker, LocalPreloadError> {
    let current_exe = std::env::current_exe().map_err(|_| LocalPreloadError::Unexpected)?;
    let mut child = Command::new(current_exe)
        .arg(INTERNAL_LOCAL_WORKER_FLAG)
        .arg(model_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|_| LocalPreloadError::Unexpected)?;
    let writer = child
        .stdin
        .take()
        .map(|stdin| Box::new(BufWriter::new(stdin)) as WorkerWriter)
        .ok_or(LocalPreloadError::Unexpected)?;
    let reader = child
        .stdout
        .take()
        .map(|stdout| Box::new(BufReader::new(stdout)) as WorkerReader)
        .ok_or(LocalPreloadError::Unexpected)?;
    let owner: SharedLocalWorkerOwner = Arc::new(WorkerProcessOwner::new(child));

    Ok(LoadingLocalWorker {
        owner,
        writer: Some(writer),
        reader: Some(reader),
    })
}

struct LocalWorkerClient {
    owner: SharedLocalWorkerOwner,
    writer: Arc<Mutex<WorkerWriter>>,
    reader: Mutex<WorkerReader>,
    inference_gate: Mutex<()>,
    next_request_id: AtomicU64,
}

fn lock_inference_gate_after_abort_check<'a>(
    gate: &'a Mutex<()>,
    abort_requested: &AtomicBool,
) -> Result<parking_lot::MutexGuard<'a, ()>, LocalTranscriptionError> {
    let guard = gate.lock();
    if abort_requested.load(Ordering::SeqCst) {
        return Err(LocalTranscriptionError::Inference);
    }
    Ok(guard)
}

impl LocalInferenceClient for LocalWorkerClient {
    fn is_alive(&self) -> bool {
        self.owner.is_alive()
    }

    fn transcribe(
        &self,
        audio: &[f32],
        language: Option<&str>,
        abort_requested: Arc<AtomicBool>,
    ) -> Result<Option<String>, LocalTranscriptionError> {
        self.transcribe_stream(audio, language, abort_requested, &mut |_| Ok(()))
    }

    fn transcribe_stream(
        &self,
        audio: &[f32],
        language: Option<&str>,
        abort_requested: Arc<AtomicBool>,
        on_segment: &mut dyn FnMut(String) -> Result<(), LocalTranscriptionError>,
    ) -> Result<Option<String>, LocalTranscriptionError> {
        if abort_requested.load(Ordering::SeqCst) {
            return Err(LocalTranscriptionError::Inference);
        }
        if audio.len() > MAX_WORKER_SAMPLES {
            return Err(LocalTranscriptionError::InferenceProtocol);
        }

        let _gate =
            lock_inference_gate_after_abort_check(&self.inference_gate, abort_requested.as_ref())?;
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut writer = self.writer.lock();
            if write_infer_command(writer.as_mut(), request_id, language, audio).is_err() {
                self.owner.terminate_and_reap();
                return Err(LocalTranscriptionError::Inference);
            }
        }

        let done = Arc::new(AtomicBool::new(false));
        let watcher_done = Arc::clone(&done);
        let watcher_abort = Arc::clone(&abort_requested);
        let watcher_writer = Arc::clone(&self.writer);
        let abort_watcher = std::thread::spawn(move || {
            while !watcher_done.load(Ordering::Acquire) && !watcher_abort.load(Ordering::SeqCst) {
                std::thread::sleep(ABORT_POLL_INTERVAL);
            }
            if watcher_abort.load(Ordering::SeqCst) && !watcher_done.load(Ordering::Acquire) {
                let mut writer = watcher_writer.lock();
                let _ = write_abort_command(writer.as_mut(), request_id);
            }
        });

        let mut reader = self.reader.lock();
        let mut full_collected: Option<String> = None;
        let mut fatal_ipc_error = false;
        let stream_result = loop {
            let chunk_response = read_inference_stream_frame(reader.as_mut());
            match chunk_response {
                Ok((resp_id, WorkerStreamFrame::Chunk(text))) => {
                    if resp_id != request_id {
                        fatal_ipc_error = true;
                        break Err(LocalTranscriptionError::InferenceProtocol);
                    }
                    if !text.is_empty() {
                        if let Some(c) = &mut full_collected {
                            c.push_str(&text);
                        } else {
                            full_collected = Some(text.clone());
                        }
                        if let Err(err) = on_segment(text) {
                            fatal_ipc_error = true;
                            break Err(err);
                        }
                    }
                }
                Ok((resp_id, WorkerStreamFrame::Done)) => {
                    if resp_id != request_id {
                        fatal_ipc_error = true;
                        break Err(LocalTranscriptionError::InferenceProtocol);
                    }
                    break Ok(full_collected);
                }
                Ok((resp_id, WorkerStreamFrame::Single(opt_text))) => {
                    if resp_id != request_id {
                        fatal_ipc_error = true;
                        break Err(LocalTranscriptionError::InferenceProtocol);
                    }
                    if full_collected.is_none() {
                        if let Some(text) = opt_text.clone() {
                            if !text.is_empty() {
                                if let Err(err) = on_segment(text) {
                                    fatal_ipc_error = true;
                                    break Err(err);
                                }
                            }
                        }
                    }
                    break Ok(opt_text);
                }
                Ok((resp_id, WorkerStreamFrame::Error(err))) => {
                    if resp_id != request_id {
                        fatal_ipc_error = true;
                        break Err(LocalTranscriptionError::InferenceProtocol);
                    }
                    break Err(err);
                }
                Err(_) => {
                    fatal_ipc_error = true;
                    break Err(LocalTranscriptionError::Inference);
                }
            }
        };

        done.store(true, Ordering::Release);
        let _ = abort_watcher.join();

        if fatal_ipc_error {
            self.owner.terminate_and_reap();
        }
        stream_result
    }
}

#[derive(Debug)]
enum WorkerCommand {
    Infer {
        request_id: u64,
        language: Option<String>,
        audio: Vec<f32>,
    },
    Abort {
        request_id: u64,
    },
}

struct ActiveInference {
    request_id: u64,
    abort_requested: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

enum WorkerThreadMessage {
    Chunk(String),
    Done(Result<Option<String>, LocalTranscriptionError>),
}

type WorkerTranscriber = Arc<
    dyn Fn(
            &[f32],
            Option<&str>,
            Arc<AtomicBool>,
            Duration,
            &mut dyn FnMut(String) -> Result<(), LocalTranscriptionError>,
        ) -> Result<Option<String>, LocalTranscriptionError>
        + Send
        + Sync,
>;

fn run_local_worker(model_path: &Path) -> i32 {
    super::super::configure_native_logging();
    let stdout = std::io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    let context = match load_verified_context(model_path) {
        Ok(context) => Arc::new(context),
        Err(error) => {
            let _ = write_load_error(&mut writer, error);
            return 1;
        }
    };
    let state = match context.create_state() {
        Ok(state) => Arc::new(parking_lot::Mutex::new(state)),
        Err(_) => {
            let _ = write_load_error(&mut writer, LocalPreloadError::InsufficientRam);
            return 1;
        }
    };
    if write_ready(&mut writer).is_err() {
        return 1;
    }

    let transcriber: WorkerTranscriber =
        Arc::new(move |audio, language, abort_requested, timeout, on_segment| {
            let mut state = state.lock();
            super::inference::transcribe_with_state_stream_abortable_timeout(
                &mut state,
                audio,
                language,
                abort_requested,
                timeout,
                on_segment,
            )
        });
    let (command_tx, command_rx) = mpsc::channel::<WorkerCommand>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        read_worker_commands(&mut reader, command_tx);
    });
    run_worker_command_loop(
        &mut writer,
        command_rx,
        transcriber,
        LOCAL_INFERENCE_TIMEOUT,
    )
}

fn read_worker_commands(
    reader: &mut (impl Read + ?Sized),
    command_tx: mpsc::Sender<WorkerCommand>,
) {
    loop {
        match read_worker_command(reader) {
            Ok(Some(command)) => {
                if command_tx.send(command).is_err() {
                    return;
                }
            }
            Ok(None) | Err(_) => return,
        }
    }
}

fn run_worker_command_loop(
    writer: &mut (impl Write + ?Sized),
    command_rx: mpsc::Receiver<WorkerCommand>,
    transcriber: WorkerTranscriber,
    inference_timeout: Duration,
) -> i32 {
    let (result_tx, result_rx) = mpsc::channel::<(u64, WorkerThreadMessage)>();
    let mut active: Option<ActiveInference> = None;
    let mut input_closed = false;

    loop {
        while let Ok((request_id, msg)) = result_rx.try_recv() {
            match msg {
                WorkerThreadMessage::Chunk(chunk) => {
                    if let Some(current) = active.as_ref() {
                        if current.request_id == request_id {
                            if write_inference_chunk(writer, request_id, &chunk).is_err() {
                                return 1;
                            }
                        }
                    }
                }
                WorkerThreadMessage::Done(result) => {
                    let Some(current) = active.take() else {
                        if write_inference_response(
                            writer,
                            request_id,
                            Err(LocalTranscriptionError::InferenceProtocol),
                        )
                        .is_err()
                        {
                            return 1;
                        }
                        continue;
                    };
                    let _ = current.join.join();
                    if current.request_id != request_id {
                        if write_inference_response(
                            writer,
                            request_id,
                            Err(LocalTranscriptionError::InferenceProtocol),
                        )
                        .is_err()
                        {
                            return 1;
                        }
                    } else {
                        match result {
                            Ok(_) => {
                                if write_inference_done(writer, request_id).is_err() {
                                    return 1;
                                }
                            }
                            Err(err) => {
                                if write_inference_response(writer, request_id, Err(err)).is_err() {
                                    return 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        if input_closed && active.is_none() {
            return 0;
        }

        match command_rx.recv_timeout(WORKER_COMMAND_POLL_INTERVAL) {
            Ok(WorkerCommand::Infer {
                request_id,
                language,
                audio,
            }) => {
                if active.is_some() {
                    if write_inference_response(
                        writer,
                        request_id,
                        Err(LocalTranscriptionError::InferenceProtocol),
                    )
                    .is_err()
                    {
                        return 1;
                    }
                    continue;
                }
                let abort_requested = Arc::new(AtomicBool::new(false));
                let inference_abort = Arc::clone(&abort_requested);
                let inference = Arc::clone(&transcriber);
                let inference_result = result_tx.clone();
                let join = std::thread::spawn(move || {
                    let result = inference(
                        &audio,
                        language.as_deref(),
                        inference_abort,
                        inference_timeout,
                        &mut |chunk| {
                            let _ = inference_result
                                .send((request_id, WorkerThreadMessage::Chunk(chunk)));
                            Ok(())
                        },
                    );
                    let _ = inference_result
                        .send((request_id, WorkerThreadMessage::Done(result)));
                });
                active = Some(ActiveInference {
                    request_id,
                    abort_requested,
                    join,
                });
            }
            Ok(WorkerCommand::Abort { request_id }) => {
                if let Some(current) = active.as_ref() {
                    if current.request_id == request_id {
                        current.abort_requested.store(true, Ordering::SeqCst);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                input_closed = true;
                if let Some(current) = active.as_ref() {
                    current.abort_requested.store(true, Ordering::SeqCst);
                }
            }
        }
    }
}

fn load_verified_context(model_path: &Path) -> Result<WhisperContext, LocalPreloadError> {
    if !model_path.is_file() {
        return Err(LocalPreloadError::ModelNotFound);
    }
    let context = create_whisper_context_with_selection(model_path)
        .map_err(|_| LocalPreloadError::UnsupportedModel)?
        .0;
    context.create_state().map_err(|error| match error {
        WhisperError::FailedToCreateState => LocalPreloadError::InsufficientRam,
        _ => LocalPreloadError::Unexpected,
    })?;
    Ok(context)
}

fn parse_internal_local_worker_args(
    args: impl IntoIterator<Item = OsString>,
) -> Option<Result<PathBuf, ()>> {
    let mut args = args.into_iter();
    let first = args.next()?;
    if first != INTERNAL_LOCAL_WORKER_FLAG {
        return None;
    }
    let Some(model_path) = args.next().map(PathBuf::from) else {
        return Some(Err(()));
    };
    if args.next().is_some() {
        return Some(Err(()));
    }
    Some(Ok(model_path))
}

/// Run the private long-lived Local Whisper worker before Tauri/app setup.
///
/// The parent app owns worker lifetime. There is deliberately no model-load
/// timeout: model/provider replacement terminates the owned child process, and
/// tray Quit terminates all Local worker ownership before application exit.
pub(crate) fn run_internal_worker_from_process_args() -> Option<i32> {
    let parsed = parse_internal_local_worker_args(std::env::args_os().skip(1))?;
    let Ok(model_path) = parsed else {
        return Some(2);
    };
    Some(run_local_worker(&model_path))
}

fn write_ready(writer: &mut (impl Write + ?Sized)) -> io::Result<()> {
    writer.write_all(&[RESPONSE_READY])?;
    writer.flush()
}

fn write_load_error(
    writer: &mut (impl Write + ?Sized),
    error: LocalPreloadError,
) -> io::Result<()> {
    writer.write_all(&[RESPONSE_LOAD_ERROR, encode_preload_error(error)])?;
    writer.flush()
}

fn write_infer_command(
    writer: &mut (impl Write + ?Sized),
    request_id: u64,
    language: Option<&str>,
    audio: &[f32],
) -> io::Result<()> {
    if audio.len() > MAX_WORKER_SAMPLES || audio.len() > u32::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "worker audio exceeds protocol limit",
        ));
    }
    let language = language.unwrap_or_default().as_bytes();
    if language.len() > MAX_WORKER_LANGUAGE_BYTES || language.len() > u32::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "worker language exceeds protocol limit",
        ));
    }

    writer.write_all(&[COMMAND_INFER])?;
    write_u64(writer, request_id)?;
    write_u32(writer, language.len() as u32)?;
    writer.write_all(language)?;
    write_u32(writer, audio.len() as u32)?;
    let mut bytes = Vec::with_capacity(audio.len() * std::mem::size_of::<f32>());
    for sample in audio {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    writer.write_all(&bytes)?;
    writer.flush()
}

fn write_abort_command(writer: &mut (impl Write + ?Sized), request_id: u64) -> io::Result<()> {
    writer.write_all(&[COMMAND_ABORT])?;
    write_u64(writer, request_id)?;
    writer.flush()
}

fn read_worker_command(reader: &mut (impl Read + ?Sized)) -> io::Result<Option<WorkerCommand>> {
    let mut tag = [0u8; 1];
    match reader.read(&mut tag)? {
        0 => return Ok(None),
        1 => {}
        _ => unreachable!(),
    }
    match tag[0] {
        COMMAND_INFER => {
            let request_id = read_u64(reader)?;
            let language_len = read_u32(reader)? as usize;
            if language_len > MAX_WORKER_LANGUAGE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "worker language exceeds limit",
                ));
            }
            let mut language_bytes = vec![0u8; language_len];
            reader.read_exact(&mut language_bytes)?;
            let language = if language_bytes.is_empty() {
                None
            } else {
                Some(String::from_utf8(language_bytes).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "worker language is not UTF-8")
                })?)
            };
            let sample_count = read_u32(reader)? as usize;
            if sample_count > MAX_WORKER_SAMPLES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "worker sample count exceeds limit",
                ));
            }
            let byte_len = sample_count.checked_mul(4).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "worker sample byte length overflow",
                )
            })?;
            let mut bytes = vec![0u8; byte_len];
            reader.read_exact(&mut bytes)?;
            let audio = bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();
            Ok(Some(WorkerCommand::Infer {
                request_id,
                language,
                audio,
            }))
        }
        COMMAND_ABORT => Ok(Some(WorkerCommand::Abort {
            request_id: read_u64(reader)?,
        })),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unknown Local worker command",
        )),
    }
}

fn write_inference_response(
    writer: &mut (impl Write + ?Sized),
    request_id: u64,
    result: Result<Option<String>, LocalTranscriptionError>,
) -> io::Result<()> {
    writer.write_all(&[RESPONSE_INFERENCE])?;
    write_u64(writer, request_id)?;
    match result {
        Ok(None) => writer.write_all(&[INFERENCE_NONE])?,
        Ok(Some(text)) => {
            let bytes = text.as_bytes();
            if bytes.len() > MAX_WORKER_TEXT_BYTES || bytes.len() > u32::MAX as usize {
                writer.write_all(&[
                    INFERENCE_ERROR,
                    encode_local_error(LocalTranscriptionError::InferenceProtocol),
                ])?;
            } else {
                writer.write_all(&[INFERENCE_TEXT])?;
                write_u32(writer, bytes.len() as u32)?;
                writer.write_all(bytes)?;
            }
        }
        Err(error) => {
            writer.write_all(&[INFERENCE_ERROR, encode_local_error(error)])?;
        }
    }
    writer.flush()
}

#[cfg(test)]
fn read_inference_response(
    reader: &mut (impl Read + ?Sized),
) -> io::Result<(u64, Result<Option<String>, LocalTranscriptionError>)> {
    if read_u8(reader)? != RESPONSE_INFERENCE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected Local worker response",
        ));
    }
    let request_id = read_u64(reader)?;
    let result = match read_u8(reader)? {
        INFERENCE_NONE => Ok(None),
        INFERENCE_TEXT => {
            let len = read_u32(reader)? as usize;
            if len > MAX_WORKER_TEXT_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Local worker text exceeds limit",
                ));
            }
            let mut bytes = vec![0u8; len];
            reader.read_exact(&mut bytes)?;
            let text = String::from_utf8(bytes).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "Local worker text is not UTF-8")
            })?;
            Ok(Some(text))
        }
        INFERENCE_ERROR => {
            let code = read_u8(reader)?;
            Err(decode_local_error(code).unwrap_or(LocalTranscriptionError::InferenceProtocol))
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown Local worker inference result",
            ))
        }
    };
    Ok((request_id, result))
}


#[derive(Debug, PartialEq, Eq)]
enum WorkerStreamFrame {
    Chunk(String),
    Done,
    Single(Option<String>),
    Error(LocalTranscriptionError),
}

fn write_inference_chunk(
    writer: &mut (impl Write + ?Sized),
    request_id: u64,
    chunk: &str,
) -> io::Result<()> {
    writer.write_all(&[RESPONSE_INFERENCE])?;
    write_u64(writer, request_id)?;
    let bytes = chunk.as_bytes();
    if bytes.len() > MAX_WORKER_TEXT_BYTES || bytes.len() > u32::MAX as usize {
        writer.write_all(&[
            INFERENCE_ERROR,
            encode_local_error(LocalTranscriptionError::InferenceProtocol),
        ])?;
    } else {
        writer.write_all(&[INFERENCE_CHUNK])?;
        write_u32(writer, bytes.len() as u32)?;
        writer.write_all(bytes)?;
    }
    writer.flush()
}

fn write_inference_done(
    writer: &mut (impl Write + ?Sized),
    request_id: u64,
) -> io::Result<()> {
    writer.write_all(&[RESPONSE_INFERENCE])?;
    write_u64(writer, request_id)?;
    writer.write_all(&[INFERENCE_DONE])?;
    writer.flush()
}

fn read_inference_stream_frame(
    reader: &mut (impl Read + ?Sized),
) -> io::Result<(u64, WorkerStreamFrame)> {
    if read_u8(reader)? != RESPONSE_INFERENCE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected Local worker response",
        ));
    }
    let request_id = read_u64(reader)?;
    let frame = match read_u8(reader)? {
        INFERENCE_NONE => WorkerStreamFrame::Single(None),
        INFERENCE_TEXT => {
            let len = read_u32(reader)? as usize;
            if len > MAX_WORKER_TEXT_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Local worker text exceeds limit",
                ));
            }
            let mut bytes = vec![0u8; len];
            reader.read_exact(&mut bytes)?;
            let text = String::from_utf8(bytes).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "Local worker text is not UTF-8")
            })?;
            WorkerStreamFrame::Single(Some(text))
        }
        INFERENCE_CHUNK => {
            let len = read_u32(reader)? as usize;
            if len > MAX_WORKER_TEXT_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Local worker chunk exceeds limit",
                ));
            }
            let mut bytes = vec![0u8; len];
            reader.read_exact(&mut bytes)?;
            let text = String::from_utf8(bytes).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "Local worker chunk is not UTF-8")
            })?;
            WorkerStreamFrame::Chunk(text)
        }
        INFERENCE_DONE => WorkerStreamFrame::Done,
        INFERENCE_ERROR => {
            let code = read_u8(reader)?;
            WorkerStreamFrame::Error(
                decode_local_error(code).unwrap_or(LocalTranscriptionError::InferenceProtocol),
            )
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown Local worker inference result",
            ))
        }
    };
    Ok((request_id, frame))
}
fn encode_preload_error(error: LocalPreloadError) -> u8 {
    match error {
        LocalPreloadError::ModelNotFound => 1,
        LocalPreloadError::UnsupportedModel => 2,
        LocalPreloadError::InsufficientRam => 3,
        LocalPreloadError::Unexpected => 4,
    }
}

fn decode_preload_error(code: u8) -> Option<LocalPreloadError> {
    match code {
        1 => Some(LocalPreloadError::ModelNotFound),
        2 => Some(LocalPreloadError::UnsupportedModel),
        3 => Some(LocalPreloadError::InsufficientRam),
        4 => Some(LocalPreloadError::Unexpected),
        _ => None,
    }
}

fn encode_local_error(error: LocalTranscriptionError) -> u8 {
    match error {
        LocalTranscriptionError::StateCreation => 1,
        LocalTranscriptionError::Inference => 2,
        LocalTranscriptionError::SegmentRead => 3,
        _ => 4,
    }
}

fn decode_local_error(code: u8) -> Option<LocalTranscriptionError> {
    match code {
        1 => Some(LocalTranscriptionError::StateCreation),
        2 => Some(LocalTranscriptionError::Inference),
        3 => Some(LocalTranscriptionError::SegmentRead),
        4 => Some(LocalTranscriptionError::InferenceProtocol),
        _ => None,
    }
}

fn write_u32(writer: &mut (impl Write + ?Sized), value: u32) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn read_u32(reader: &mut (impl Read + ?Sized)) -> io::Result<u32> {
    let mut bytes = [0u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn write_u64(writer: &mut (impl Write + ?Sized), value: u64) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn read_u64(reader: &mut (impl Read + ?Sized)) -> io::Result<u64> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_u8(reader: &mut (impl Read + ?Sized)) -> io::Result<u8> {
    let mut byte = [0u8; 1];
    reader.read_exact(&mut byte)?;
    Ok(byte[0])
}

#[cfg(test)]
struct ThreadWorkerOwner {
    control: Mutex<Option<std::net::TcpStream>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

#[cfg(test)]
impl ThreadWorkerOwner {
    fn terminate_and_reap_inner(&self) {
        if let Some(control) = self.control.lock().take() {
            let _ = control.shutdown(std::net::Shutdown::Both);
        }
        if let Some(join) = self.join.lock().take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
impl LocalWorkerOwner for ThreadWorkerOwner {
    fn terminate_and_reap(&self) {
        self.terminate_and_reap_inner();
    }
}

#[cfg(test)]
impl Drop for ThreadWorkerOwner {
    fn drop(&mut self) {
        self.terminate_and_reap_inner();
    }
}

#[cfg(test)]
fn connected_worker_streams() -> io::Result<(std::net::TcpStream, std::net::TcpStream)> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;
    let client = std::net::TcpStream::connect(address)?;
    let (server, _) = listener.accept()?;
    Ok((client, server))
}

#[cfg(test)]
fn loading_worker_from_streams(
    client: std::net::TcpStream,
    owner: SharedLocalWorkerOwner,
) -> io::Result<LoadingLocalWorker> {
    let writer = Box::new(BufWriter::new(client.try_clone()?)) as WorkerWriter;
    let reader = Box::new(BufReader::new(client)) as WorkerReader;
    Ok(LoadingLocalWorker {
        owner,
        writer: Some(writer),
        reader: Some(reader),
    })
}

#[cfg(test)]
fn command_channel_from_stream(reader: std::net::TcpStream) -> mpsc::Receiver<WorkerCommand> {
    let (command_tx, command_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        read_worker_commands(&mut reader, command_tx);
    });
    command_rx
}

#[cfg(test)]
fn spawn_in_process_local_worker(
    model_path: &Path,
    timeout_override: Option<Arc<Mutex<Duration>>>,
) -> Result<LoadingLocalWorker, LocalPreloadError> {
    let (client, server) = connected_worker_streams().map_err(|_| LocalPreloadError::Unexpected)?;
    let control = client
        .try_clone()
        .map_err(|_| LocalPreloadError::Unexpected)?;
    let model_path = model_path.to_path_buf();
    let join = std::thread::spawn(move || {
        let reader = match server.try_clone() {
            Ok(reader) => reader,
            Err(_) => return,
        };
        let mut writer = BufWriter::new(server);
        let context = match load_verified_context(&model_path) {
            Ok(context) => Arc::new(context),
            Err(error) => {
                let _ = write_load_error(&mut writer, error);
                return;
            }
        };
        if write_ready(&mut writer).is_err() {
            return;
        }
        let transcriber: WorkerTranscriber =
            Arc::new(move |audio, language, abort_requested, timeout, on_segment| {
                let timeout = timeout_override
                    .as_ref()
                    .map(|override_timeout| *override_timeout.lock())
                    .unwrap_or(timeout);
                super::inference::transcribe_with_context_stream_abortable_timeout(
                    &context,
                    audio,
                    language,
                    abort_requested,
                    timeout,
                    on_segment,
                )
            });
        let command_rx = command_channel_from_stream(reader);
        let _ = run_worker_command_loop(
            &mut writer,
            command_rx,
            transcriber,
            LOCAL_INFERENCE_TIMEOUT,
        );
    });
    let owner: SharedLocalWorkerOwner = Arc::new(ThreadWorkerOwner {
        control: Mutex::new(Some(control)),
        join: Mutex::new(Some(join)),
    });
    loading_worker_from_streams(client, owner).map_err(|_| LocalPreloadError::Unexpected)
}

#[cfg(test)]
pub(super) fn in_process_worker_spawner(
    timeout_override: Option<Arc<Mutex<Duration>>>,
) -> LocalWorkerSpawner {
    Arc::new(move |model_path| spawn_in_process_local_worker(model_path, timeout_override.clone()))
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct ControlledWorkerProbe {
    path: PathBuf,
    state: Arc<(Mutex<ControlledWorkerState>, parking_lot::Condvar)>,
}

#[cfg(test)]
enum ControlledWorkerOutcome {
    Ready,
    LoadError(LocalPreloadError),
}

#[cfg(test)]
#[derive(Default)]
struct ControlledWorkerState {
    outcome: Option<ControlledWorkerOutcome>,
    terminated: bool,
    reaped: bool,
}

#[cfg(test)]
impl ControlledWorkerProbe {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn complete_ready(&self) {
        self.complete(ControlledWorkerOutcome::Ready);
    }

    pub(crate) fn complete_error(&self, error: LocalPreloadError) {
        self.complete(ControlledWorkerOutcome::LoadError(error));
    }

    fn complete(&self, outcome: ControlledWorkerOutcome) {
        let (state, changed) = &*self.state;
        let mut state = state.lock();
        assert!(
            !state.terminated && state.outcome.is_none(),
            "controlled worker completion must be unique and precede termination"
        );
        state.outcome = Some(outcome);
        changed.notify_all();
    }

    pub(crate) fn is_terminated(&self) -> bool {
        self.state.0.lock().terminated
    }

    pub(crate) fn wait_until_reaped(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let (state, changed) = &*self.state;
        let mut state = state.lock();
        while !state.reaped {
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                return false;
            };
            if changed.wait_for(&mut state, remaining).timed_out() && !state.reaped {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
struct ControlledWorkerOwner {
    state: Arc<(Mutex<ControlledWorkerState>, parking_lot::Condvar)>,
    control: Mutex<Option<std::net::TcpStream>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

#[cfg(test)]
impl ControlledWorkerOwner {
    fn terminate_and_reap_inner(&self) {
        {
            let (state, changed) = &*self.state;
            let mut state = state.lock();
            state.terminated = true;
            changed.notify_all();
        }
        if let Some(control) = self.control.lock().take() {
            let _ = control.shutdown(std::net::Shutdown::Both);
        }
        if let Some(join) = self.join.lock().take() {
            let _ = join.join();
        }
        let (state, changed) = &*self.state;
        state.lock().reaped = true;
        changed.notify_all();
    }
}

#[cfg(test)]
impl LocalWorkerOwner for ControlledWorkerOwner {
    fn terminate_and_reap(&self) {
        self.terminate_and_reap_inner();
    }
}

#[cfg(test)]
impl Drop for ControlledWorkerOwner {
    fn drop(&mut self) {
        self.terminate_and_reap_inner();
    }
}

#[cfg(test)]
fn spawn_controlled_worker(
    model_path: &Path,
    spawned: &mpsc::Sender<ControlledWorkerProbe>,
) -> Result<LoadingLocalWorker, LocalPreloadError> {
    let (client, server) = connected_worker_streams().map_err(|_| LocalPreloadError::Unexpected)?;
    let control = client
        .try_clone()
        .map_err(|_| LocalPreloadError::Unexpected)?;
    let state = Arc::new((
        Mutex::new(ControlledWorkerState::default()),
        parking_lot::Condvar::new(),
    ));
    let server_state = Arc::clone(&state);
    let join = std::thread::spawn(move || {
        let outcome = {
            let (state, changed) = &*server_state;
            let mut state = state.lock();
            while state.outcome.is_none() && !state.terminated {
                changed.wait(&mut state);
            }
            if state.terminated {
                return;
            }
            state.outcome.take()
        };
        let reader = match server.try_clone() {
            Ok(reader) => reader,
            Err(_) => return,
        };
        let mut writer = BufWriter::new(server);
        match outcome {
            Some(ControlledWorkerOutcome::Ready) => {
                if write_ready(&mut writer).is_err() {
                    return;
                }
                let transcriber: WorkerTranscriber = Arc::new(|_, _, abort_requested, _, _| {
                    if abort_requested.load(Ordering::SeqCst) {
                        Err(LocalTranscriptionError::Inference)
                    } else {
                        Ok(None)
                    }
                });
                let command_rx = command_channel_from_stream(reader);
                let _ = run_worker_command_loop(
                    &mut writer,
                    command_rx,
                    transcriber,
                    LOCAL_INFERENCE_TIMEOUT,
                );
            }
            Some(ControlledWorkerOutcome::LoadError(error)) => {
                let _ = write_load_error(&mut writer, error);
            }
            None => {}
        }
    });
    let owner: SharedLocalWorkerOwner = Arc::new(ControlledWorkerOwner {
        state: Arc::clone(&state),
        control: Mutex::new(Some(control)),
        join: Mutex::new(Some(join)),
    });
    let worker =
        loading_worker_from_streams(client, owner).map_err(|_| LocalPreloadError::Unexpected)?;
    let _ = spawned.send(ControlledWorkerProbe {
        path: model_path.to_path_buf(),
        state,
    });
    Ok(worker)
}

#[cfg(test)]
pub(super) fn controlled_worker_spawner(
) -> (LocalWorkerSpawner, mpsc::Receiver<ControlledWorkerProbe>) {
    let (spawned_tx, spawned_rx) = mpsc::channel();
    let spawner =
        Arc::new(move |model_path: &Path| spawn_controlled_worker(model_path, &spawned_tx));
    (spawner, spawned_rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn protocol_client(
        transcriber: WorkerTranscriber,
        timeout: Duration,
    ) -> SharedLocalInferenceClient {
        let (client, server) = connected_worker_streams().unwrap();
        let control = client.try_clone().unwrap();
        let server_reader = server.try_clone().unwrap();
        let join = std::thread::spawn(move || {
            let command_rx = command_channel_from_stream(server_reader);
            let mut writer = BufWriter::new(server);
            let _ = run_worker_command_loop(&mut writer, command_rx, transcriber, timeout);
        });
        let owner: SharedLocalWorkerOwner = Arc::new(ThreadWorkerOwner {
            control: Mutex::new(Some(control)),
            join: Mutex::new(Some(join)),
        });
        Arc::new(LocalWorkerClient {
            owner,
            writer: Arc::new(Mutex::new(
                Box::new(BufWriter::new(client.try_clone().unwrap())) as WorkerWriter,
            )),
            reader: Mutex::new(Box::new(BufReader::new(client)) as WorkerReader),
            inference_gate: Mutex::new(()),
            next_request_id: AtomicU64::new(1),
        })
    }

    #[test]
    fn aborted_request_is_rejected_after_waiting_for_inference_gate() {
        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock();
        let abort_requested = Arc::new(AtomicBool::new(false));
        let waiting_gate = Arc::clone(&gate);
        let waiting_abort = Arc::clone(&abort_requested);
        let (result_tx, result_rx) = mpsc::channel();

        std::thread::spawn(move || {
            let result = lock_inference_gate_after_abort_check(
                waiting_gate.as_ref(),
                waiting_abort.as_ref(),
            )
            .map(|_| ());
            let _ = result_tx.send(result);
        });

        abort_requested.store(true, Ordering::SeqCst);
        drop(held);

        assert_eq!(
            result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(LocalTranscriptionError::Inference)
        );
    }

    #[test]
    fn explicit_abort_crosses_parent_and_worker_ipc_before_inference_returns() {
        let (started_tx, started_rx) = mpsc::channel();
        let (native_abort_tx, native_abort_rx) = mpsc::channel();
        let transcriber: WorkerTranscriber = Arc::new(move |_, _, abort_requested, _, _| {
            started_tx.send(()).unwrap();
            while !abort_requested.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            native_abort_tx.send(()).unwrap();
            Err(LocalTranscriptionError::Inference)
        });
        let client = protocol_client(transcriber, LOCAL_INFERENCE_TIMEOUT);
        let abort_requested = Arc::new(AtomicBool::new(false));
        let inference_abort = Arc::clone(&abort_requested);
        let (result_tx, result_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = result_tx.send(client.transcribe(&[0.25], Some("en"), inference_abort));
        });

        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        abort_requested.store(true, Ordering::SeqCst);

        native_abort_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker-side native abort flag must observe the IPC abort command");
        assert_eq!(
            result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(LocalTranscriptionError::Inference)
        );
    }

    #[test]
    fn aborted_request_waiting_for_shared_worker_gate_never_sends_infer() {
        let calls = Arc::new(AtomicU64::new(0));
        let observed_calls = Arc::clone(&calls);
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let release_first_rx = Arc::new(Mutex::new(release_first_rx));
        let transcriber: WorkerTranscriber = Arc::new(move |_, _, _, _, _| {
            let call = observed_calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                first_started_tx.send(()).unwrap();
                release_first_rx.lock().recv().unwrap();
            }
            Ok(None)
        });
        let client = protocol_client(transcriber, LOCAL_INFERENCE_TIMEOUT);

        let first_client = Arc::clone(&client);
        let (first_result_tx, first_result_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = first_result_tx.send(first_client.transcribe(
                &[0.25],
                Some("en"),
                Arc::new(AtomicBool::new(false)),
            ));
        });
        first_started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        let second_abort = Arc::new(AtomicBool::new(false));
        let second_abort_for_call = Arc::clone(&second_abort);
        let second_client = Arc::clone(&client);
        let (second_result_tx, second_result_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = second_result_tx.send(second_client.transcribe(
                &[0.5],
                Some("en"),
                second_abort_for_call,
            ));
        });

        second_abort.store(true, Ordering::SeqCst);
        release_first_tx.send(()).unwrap();

        assert_eq!(
            first_result_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            Ok(None)
        );
        assert_eq!(
            second_result_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            Err(LocalTranscriptionError::Inference)
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "an aborted request that waited for the shared worker gate must never send a new Infer command"
        );
    }

    #[test]
    fn worker_ipc_applies_thirty_second_native_inference_policy() {
        let (timeout_tx, timeout_rx) = mpsc::channel();
        let transcriber: WorkerTranscriber = Arc::new(move |_, _, _, timeout, _| {
            timeout_tx.send(timeout).unwrap();
            Err(LocalTranscriptionError::Inference)
        });
        let client = protocol_client(transcriber, LOCAL_INFERENCE_TIMEOUT);

        assert_eq!(
            client.transcribe(&[0.25], Some("en"), Arc::new(AtomicBool::new(false))),
            Err(LocalTranscriptionError::Inference)
        );
        assert_eq!(
            timeout_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn worker_streaming_transcription_emits_chunks_once_without_duplicate_single_response() {
        let transcriber: WorkerTranscriber = Arc::new(move |_, _, _, _, on_segment| {
            on_segment("sample segment".to_string())?;
            Ok(Some("sample segment".to_string()))
        });
        let client = protocol_client(transcriber, LOCAL_INFERENCE_TIMEOUT);
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emitted_clone = Arc::clone(&emitted);

        let result = client.transcribe_stream(
            &[0.25],
            Some("en"),
            Arc::new(AtomicBool::new(false)),
            &mut |chunk| {
                emitted_clone.lock().push(chunk);
                Ok(())
            },
        );

        assert_eq!(result, Ok(Some("sample segment".to_string())));
        assert_eq!(
            &*emitted.lock(),
            &["sample segment".to_string()],
            "chunk must be emitted exactly once to on_segment"
        );
    }

    #[test]
    fn internal_worker_parser_accepts_only_exact_mode_and_path() {
        assert_eq!(
            parse_internal_local_worker_args([
                OsString::from(INTERNAL_LOCAL_WORKER_FLAG),
                OsString::from("model.bin"),
            ]),
            Some(Ok(PathBuf::from("model.bin")))
        );
        assert_eq!(
            parse_internal_local_worker_args([OsString::from("--other")]),
            None
        );
        assert_eq!(
            parse_internal_local_worker_args([OsString::from(INTERNAL_LOCAL_WORKER_FLAG)]),
            Some(Err(()))
        );
        assert_eq!(
            parse_internal_local_worker_args([
                OsString::from(INTERNAL_LOCAL_WORKER_FLAG),
                OsString::from("model.bin"),
                OsString::from("extra"),
            ]),
            Some(Err(()))
        );
    }

    #[test]
    fn binary_infer_command_round_trips_exact_language_and_audio() {
        let audio = vec![0.25, -0.5, 1.0];
        let mut bytes = Vec::new();
        write_infer_command(&mut bytes, 7, Some("en"), &audio).unwrap();
        let command = read_worker_command(&mut Cursor::new(bytes))
            .unwrap()
            .unwrap();
        match command {
            WorkerCommand::Infer {
                request_id,
                language,
                audio: decoded,
            } => {
                assert_eq!(request_id, 7);
                assert_eq!(language.as_deref(), Some("en"));
                assert_eq!(decoded, audio);
            }
            WorkerCommand::Abort { .. } => panic!("unexpected abort command"),
        }
    }

    #[test]
    fn binary_protocol_rejects_oversized_language_before_allocation() {
        let mut bytes = Vec::new();
        bytes.push(COMMAND_INFER);
        write_u64(&mut bytes, 1).unwrap();
        write_u32(&mut bytes, (MAX_WORKER_LANGUAGE_BYTES + 1) as u32).unwrap();
        assert!(read_worker_command(&mut Cursor::new(bytes)).is_err());
    }

    #[test]
    fn binary_abort_command_round_trips_request_identity() {
        let mut bytes = Vec::new();
        write_abort_command(&mut bytes, 42).unwrap();
        assert!(matches!(
            read_worker_command(&mut Cursor::new(bytes)).unwrap(),
            Some(WorkerCommand::Abort { request_id: 42 })
        ));
    }

    #[test]
    fn binary_inference_responses_round_trip_none_text_and_error() {
        for (request_id, expected) in [
            (1, Ok(None)),
            (2, Ok(Some(" exact text ".to_string()))),
            (3, Err(LocalTranscriptionError::Inference)),
        ] {
            let mut bytes = Vec::new();
            write_inference_response(&mut bytes, request_id, expected.clone()).unwrap();
            assert_eq!(
                read_inference_response(&mut Cursor::new(bytes)).unwrap(),
                (request_id, expected)
            );
        }
    }

    #[test]
    fn binary_inference_stream_frames_round_trip_chunk_and_done() {
        let mut bytes = Vec::new();
        write_inference_chunk(&mut bytes, 10, " first chunk").unwrap();
        write_inference_chunk(&mut bytes, 10, " second chunk").unwrap();
        write_inference_done(&mut bytes, 10).unwrap();

        let mut cursor = Cursor::new(bytes);
        assert_eq!(
            read_inference_stream_frame(&mut cursor).unwrap(),
            (10, WorkerStreamFrame::Chunk(" first chunk".to_string()))
        );
        assert_eq!(
            read_inference_stream_frame(&mut cursor).unwrap(),
            (10, WorkerStreamFrame::Chunk(" second chunk".to_string()))
        );
        assert_eq!(
            read_inference_stream_frame(&mut cursor).unwrap(),
            (10, WorkerStreamFrame::Done)
        );
    }

    #[test]
    fn preload_error_protocol_round_trips_all_stable_variants() {
        for error in [
            LocalPreloadError::ModelNotFound,
            LocalPreloadError::UnsupportedModel,
            LocalPreloadError::InsufficientRam,
            LocalPreloadError::Unexpected,
        ] {
            assert_eq!(
                decode_preload_error(encode_preload_error(error)),
                Some(error)
            );
        }
    }
}
