//! Supervision and terminal presentation for one locally launched workflow.

use crate::{
    duration::format_duration_ns,
    graph::{ConnectionView, GraphCluster, GraphError, GraphLayout, GraphRect, GraphView},
    logs::LogSnapshot,
    manifest::{ManifestReadError, read_manifest},
    receiver::{LoopbackReceiver, ObservationReader, ReceiverError},
    state::{LoopPassObservation, NodeObservation, NodeStatus, StateSnapshot},
    traces::{TraceSnapshot, TraceView, TraceViewSnapshot},
};
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use mf_runtime::{MAX_WORKFLOW_INPUT_BYTES, WorkflowArguments, WorkflowInputError};
use mf_telemetry::{description::WorkflowDescription, identity::RunId};
use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    sys::signal::Signal,
};
use process_wrap::std::{ChildWrapper, CommandWrap, ProcessGroup};
use ratatui::{
    Frame, Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph},
};
use signal_hook::{SigId, consts::SIGINT, flag, low_level::unregister};
use snafu::{IntoError, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    env,
    fs::File,
    io::{self, IsTerminal, Read, Seek, SeekFrom, Write},
    os::{
        fd::AsFd,
        unix::{fs::OpenOptionsExt, process::ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_STDOUT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 1024 * 1024;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Snafu)]
pub enum RunError {
    #[snafu(transparent)]
    StreamTransport { source: mf_runtime::StreamError },
    #[snafu(display("TUI requires terminal stdin and stderr"))]
    TerminalRequired,
    #[snafu(transparent)]
    ManifestContract {
        source: mf_runtime::WorkflowManifestError,
    },
    #[snafu(transparent)]
    Inputs { source: WorkflowInputError },
    #[snafu(transparent)]
    Arguments {
        source: mf_runtime::RunnerArgumentError,
    },
    #[snafu(display("{message}"))]
    Options { message: String },
    #[snafu(display("could not read workflow input file {path:?}: {source}"))]
    InputFile { path: PathBuf, source: io::Error },
    #[snafu(display("could not prepare private workflow arguments: {source}"))]
    ArgumentFile { source: io::Error },
    #[snafu(display("could not inspect workflow: {source}"))]
    Manifest { source: ManifestReadError },
    #[snafu(display("could not layout workflow graph: {source}"))]
    Graph { source: GraphError },
    #[snafu(display("could not create private stdout spool: {source}"))]
    Spool { source: io::Error },
    #[snafu(display("could not start local telemetry receiver: {source}"))]
    Receiver { source: ReceiverError },
    #[snafu(display("could not initialize terminal or receive input: {source}"))]
    Terminal { source: io::Error },
    #[snafu(display("could not launch workflow {path:?}: {source}"))]
    Spawn { path: PathBuf, source: io::Error },
    #[snafu(display("could not {operation}: {source}"))]
    Supervise {
        operation: &'static str,
        source: io::Error,
    },
}

#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    /// Local diagnostic detail, capped at three.
    pub verbose: u8,
    pub inputs: Option<String>,
    pub inputs_file: Option<PathBuf>,
    pub arguments: Vec<std::ffi::OsString>,
}

struct PreparedLaunch {
    description: WorkflowDescription,
    arguments: Option<tempfile::NamedTempFile>,
}

fn option_error<T>(message: &str) -> Result<T, RunError> {
    OptionsSnafu {
        message: message.to_owned(),
    }
    .fail()
}

fn open_input_file(path: &Path) -> Result<File, RunError> {
    let open = || -> io::Result<File> {
        let file = File::options()
            .read(true)
            .custom_flags(OFlag::O_NONBLOCK.bits())
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a regular file",
            ));
        }
        Ok(file)
    };
    open().context(InputFileSnafu {
        path: path.to_owned(),
    })
}

impl RunOptions {
    pub fn workflow_arguments(
        &self,
        schema: &mf_runtime::WorkflowInputSchema,
    ) -> Result<WorkflowArguments, RunError> {
        if self.inputs.is_some() && self.inputs_file.is_some() {
            return option_error("--inputs and --inputs-file are mutually exclusive");
        }
        if !self.arguments.is_empty() && (self.inputs.is_some() || self.inputs_file.is_some()) {
            return option_error(
                "named workflow inputs cannot be combined with --inputs or --inputs-file",
            );
        }
        let arguments = if !self.arguments.is_empty() {
            WorkflowArguments::parse(self.arguments.clone(), schema)?
        } else if let Some(json) = &self.inputs {
            WorkflowArguments::from_json(json.as_bytes())?
        } else if let Some(path) = &self.inputs_file {
            let mut bytes = Vec::new();
            open_input_file(path)?
                .take(MAX_WORKFLOW_INPUT_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .context(InputFileSnafu { path: path.clone() })?;
            WorkflowArguments::from_json(&bytes)?
        } else {
            WorkflowArguments::default()
        };
        schema.validate(&arguments)?;
        Ok(arguments)
    }
}

fn prepare_launch(path: &Path, options: &RunOptions) -> Result<PreparedLaunch, RunError> {
    let manifest = read_manifest(path).context(ManifestSnafu)?;
    let arguments = options.workflow_arguments(&manifest.input_schema)?;
    let description = manifest.description()?;
    let needs_stdin = manifest.input_schema.stdin_owner(&arguments)?.is_some();
    if needs_stdin {
        return option_error(
            "TUI execution cannot supply workflow stdin; provide source parameters or run the executable directly",
        );
    }
    let arguments = if !arguments.0.is_empty() {
        let bytes = serde_json::to_vec(&arguments).context(mf_runtime::WorkflowInputJsonSnafu)?;
        if bytes.len() > MAX_WORKFLOW_INPUT_BYTES {
            mf_runtime::WorkflowInputTooLargeSnafu {
                limit: MAX_WORKFLOW_INPUT_BYTES,
            }
            .fail()?;
        }
        let mut file = tempfile::NamedTempFile::new().context(ArgumentFileSnafu)?;
        file.write_all(&bytes)
            .and_then(|()| file.flush())
            .context(ArgumentFileSnafu)?;
        Some(file)
    } else {
        None
    };
    Ok(PreparedLaunch {
        description,
        arguments,
    })
}

pub fn run_executable(path: &Path) -> Result<u8, RunError> {
    run_executable_with_options(path, &RunOptions::default())
}

pub fn run_executable_with_options(path: &Path, options: &RunOptions) -> Result<u8, RunError> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return TerminalRequiredSnafu.fail();
    }
    let area = terminal_area().context(TerminalSnafu)?;
    let PreparedLaunch {
        description,
        arguments,
    } = prepare_launch(path, options)?;
    let mut view = ObservationView::new(&description)?;
    let signal = SignalGuard::register().context(TerminalSnafu)?;
    let guard = TerminalGuard::enter().context(TerminalSnafu)?;
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(Box::new(io::stderr()) as Box<dyn Write>),
        TerminalOptions {
            viewport: Viewport::Fixed(area),
        },
    )
    .context(TerminalSnafu)?;
    let result = (|| {
        let mut command = Command::new(path);
        for _ in 0..options.verbose.min(3) {
            command.arg("--verbose");
        }
        if let Some(arguments) = &arguments {
            command.arg("--inputs-file").arg(arguments.path());
        }
        let mut run = LocalRun::start(command, description, RunId::new(), MAX_STDOUT_BYTES)?;
        supervise(&mut run, &mut view, &mut terminal, &signal)
    })();
    *terminal.backend_mut() = CrosstermBackend::new(Box::new(io::sink()));
    drop(terminal);
    drop(guard);
    drop(signal);
    let outcome = result?;
    if let Some(error) = &outcome.capture_error {
        report(&format!("workflow stdout capture incomplete: {error}"));
    }
    let mut stdout = io::stdout().lock();
    let delivery = stdout
        .write_all(&outcome.stdout)
        .and_then(|()| stdout.flush());
    if let Err(error) = &delivery {
        report(&format!(
            "could not deliver captured workflow stdout: {error}"
        ));
    }
    if outcome.interrupted {
        return Ok(130);
    }
    let code = exit_code(outcome.status);
    Ok(if outcome.capture_error.is_some() || delivery.is_err() {
        if outcome.status.success() { 1 } else { code }
    } else {
        code
    })
}

#[derive(Clone)]
pub struct LocalObservation {
    pub reader: ObservationReader,
    capture: Arc<Mutex<Capture>>,
    started: Instant,
    completion: Arc<Mutex<Option<(ExitStatus, Instant)>>>,
}
impl LocalObservation {
    pub fn status(&self) -> Option<ExitStatus> {
        self.completion
            .lock()
            .expect("run completion was not poisoned")
            .map(|(status, _)| status)
    }
    pub fn elapsed(&self) -> Duration {
        self.completion
            .lock()
            .expect("run completion was not poisoned")
            .map_or_else(
                || self.started.elapsed(),
                |(_, at)| at.duration_since(self.started),
            )
    }
    pub fn capture_view(&self) -> CaptureView {
        self.capture
            .lock()
            .expect("capture lock was not poisoned")
            .view()
    }
}
pub struct LocalOutcome {
    pub status: ExitStatus,
    pub interrupted: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub capture_error: Option<String>,
    pub capture_failure: Option<Arc<CaptureFailure>>,
    pub call_events: mf_chat::CallEventCollector,
}

#[derive(Debug, Snafu)]
pub enum CaptureFailure {
    #[snafu(display("invalid workflow output record: {source}"))]
    OutputRecord { source: serde_json::Error },
    #[snafu(display("workflow output consumer disconnected: {source}"))]
    OutputDelivery {
        source: std::sync::mpsc::TrySendError<mf_runtime::NodeValues>,
    },
    #[snafu(display("workflow output capture cancelled: {source}"))]
    OutputCancelled {
        #[snafu(source(from(mf_runtime::StreamError, Arc::new)))]
        source: Arc<mf_runtime::StreamError>,
    },
    #[snafu(display("workflow output capture stopped"))]
    OutputStopped,
    #[snafu(display("stdout {operation} failed: {source}"))]
    Io {
        operation: &'static str,
        #[snafu(source(from(io::Error, Arc::new)))]
        source: Arc<io::Error>,
    },
    #[snafu(display("stdout exceeded the {limit}-byte spool limit"))]
    SpoolLimit { limit: u64 },
    #[snafu(display("stdout pipe remained open after the drain deadline"))]
    OpenPipe,
}

#[derive(Default)]
struct NativeOutputDecoder {
    pending: Vec<u8>,
}

impl NativeOutputDecoder {
    fn feed(
        &mut self,
        bytes: &[u8],
        emit: &mut impl FnMut(mf_runtime::NodeValues) -> Result<(), CaptureFailure>,
    ) -> Result<(), CaptureFailure> {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<_> = self.pending.drain(..=end).collect();
            Self::record(&line, emit)?;
        }
        Ok(())
    }

    fn finish(
        &mut self,
        emit: &mut impl FnMut(mf_runtime::NodeValues) -> Result<(), CaptureFailure>,
    ) -> Result<(), CaptureFailure> {
        Self::record(&std::mem::take(&mut self.pending), emit)
    }

    fn record(
        bytes: &[u8],
        emit: &mut impl FnMut(mf_runtime::NodeValues) -> Result<(), CaptureFailure>,
    ) -> Result<(), CaptureFailure> {
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        emit(mf_runtime::decode_unique_json(bytes).context(OutputRecordSnafu)?)
    }
}

struct StreamCaptureState {
    decoder: NativeOutputDecoder,
    sender: std::sync::mpsc::SyncSender<mf_runtime::NodeValues>,
    cancellation: mf_runtime::StreamCancellation,
}
pub struct LocalRun {
    records: Option<std::sync::mpsc::Receiver<mf_runtime::NodeValues>>,
    child: ChildGuard,
    capture: CaptureWorker,
    receiver: LoopbackReceiver,
    started: Instant,
    exited: Option<(ExitStatus, Instant)>,
    interrupted_at: Option<Instant>,
    killed: bool,
    kill_error: Option<(io::Error, Instant)>,
    finished: bool,
    completion: Arc<Mutex<Option<(ExitStatus, Instant)>>>,
}
impl LocalRun {
    pub fn start(
        command: Command,
        description: WorkflowDescription,
        run_id: RunId,
        stdout_limit: u64,
    ) -> Result<Self, RunError> {
        Self::start_inner(command, description, run_id, stdout_limit, None)
    }

    pub fn start_stream(
        command: Command,
        description: WorkflowDescription,
        run_id: RunId,
        cancellation: mf_runtime::StreamCancellation,
    ) -> Result<Self, RunError> {
        Self::start_inner(
            command,
            description,
            run_id,
            MAX_STDOUT_BYTES,
            Some(cancellation),
        )
    }

    fn start_inner(
        mut command: Command,
        description: WorkflowDescription,
        run_id: RunId,
        stdout_limit: u64,
        stream: Option<mf_runtime::StreamCancellation>,
    ) -> Result<Self, RunError> {
        let (records, stream) = stream
            .map(|cancellation| {
                let (sender, receiver) = std::sync::mpsc::sync_channel(64);
                (
                    receiver,
                    StreamCaptureState {
                        decoder: NativeOutputDecoder::default(),
                        sender,
                        cancellation,
                    },
                )
            })
            .unzip();
        if let Some(error) = stream
            .as_ref()
            .and_then(|stream| stream.cancellation.failure())
        {
            return Err(error.into());
        }
        let receiver =
            LoopbackReceiver::bind(description.clone(), run_id).context(ReceiverSnafu)?;
        let capture = Arc::new(Mutex::new(Capture::new().context(SpoolSnafu)?));
        capture.lock().expect("capture lock was not poisoned").limit =
            stdout_limit.min(MAX_STDOUT_BYTES);
        capture
            .lock()
            .expect("capture lock was not poisoned")
            .record_mode = stream.is_some();
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, _) in env::vars_os() {
            if name.to_string_lossy().starts_with("OTEL_EXPORTER_OTLP_") {
                command.env_remove(name);
            }
        }
        command
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", receiver.endpoint())
            .env("MF_RUN_ID", run_id.to_string())
            .env(mf_telemetry::SNAPSHOT_CAPTURE_ENV, "1");
        let path = PathBuf::from(command.get_program());
        let mut command = CommandWrap::from(command);
        command.wrap(ProcessGroup::leader());
        let mut child = ChildGuard::new(command.spawn().context(SpawnSnafu { path })?);
        capture
            .lock()
            .expect("capture lock was not poisoned")
            .attach(&mut child.child)
            .context(SuperviseSnafu {
                operation: "attach workflow output pipes",
            })?;
        Ok(Self {
            records,
            child,
            capture: match stream {
                Some(stream) => CaptureWorker::start_with_stream(capture, Some(stream)),
                None => CaptureWorker::start(capture),
            },
            receiver,
            started: Instant::now(),
            exited: None,
            interrupted_at: None,
            killed: false,
            kill_error: None,
            finished: false,
            completion: Arc::new(Mutex::new(None)),
        })
    }
    pub fn try_record(&self) -> Result<mf_runtime::NodeValues, std::sync::mpsc::TryRecvError> {
        self.records
            .as_ref()
            .ok_or(std::sync::mpsc::TryRecvError::Disconnected)?
            .try_recv()
    }

    pub fn observation(&self) -> LocalObservation {
        LocalObservation {
            reader: self.receiver.reader(),
            capture: Arc::clone(&self.capture.capture),
            started: self.started,
            completion: Arc::clone(&self.completion),
        }
    }
    pub fn cancel(&mut self) {
        if !self.finished && self.interrupted_at.is_none() {
            self.capture.cancel();
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(self.child.child.id() as i32),
                Signal::SIGINT,
            );
            self.interrupted_at = Some(Instant::now());
        }
    }
    pub fn poll(&mut self) -> Result<Option<LocalOutcome>, RunError> {
        if self.finished {
            return Ok(None);
        }
        if self.capture.cancelled() && self.interrupted_at.is_none() {
            self.cancel();
        }
        if self.exited.is_none()
            && let Some(status) = self.child.child.try_wait().context(SuperviseSnafu {
                operation: "poll workflow process group",
            })?
        {
            self.exited = Some((status, Instant::now()));
        }
        if self.exited.is_some() {
            self.kill_error = None;
        } else if self
            .kill_error
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() >= DRAIN_TIMEOUT)
        {
            let (source, _) = self.kill_error.take().expect("termination failure exists");
            return Err(source).context(SuperviseSnafu {
                operation: "terminate workflow process group",
            });
        }
        let view = self.capture.view();
        if ((view.error.is_some() && self.interrupted_at.is_none())
            || self
                .interrupted_at
                .is_some_and(|at| at.elapsed() >= INTERRUPT_TIMEOUT))
            && !self.killed
            && self.exited.is_none()
        {
            if let Err(error) = self.child.child.start_kill() {
                // Signalling a disappearing group can fail before its leader is ready to reap.
                self.kill_error = Some((error, Instant::now()));
            }
            self.killed = true;
        }
        let Some((status, at)) = self.exited else {
            return Ok(None);
        };
        if (view.stdout_open || view.stderr_open || self.capture.processing())
            && (at.elapsed() < DRAIN_TIMEOUT || self.capture.records_pending())
        {
            return Ok(None);
        }
        // Reap descendants even when they have already closed their inherited streams.
        let _ = self.child.child.start_kill();
        self.capture.stop();
        let mut capture = self
            .capture
            .capture
            .lock()
            .expect("capture lock was not poisoned");
        if capture.stdout.is_some() {
            capture.record_failure(OpenPipeSnafu.build());
        }
        capture.stdout = None;
        capture.stderr = None;
        let stdout = if capture.record_mode {
            Vec::new()
        } else {
            capture
                .spool
                .seek(SeekFrom::Start(0))
                .context(SuperviseSnafu {
                    operation: "seek captured workflow output",
                })?;
            let mut stdout = Vec::new();
            capture
                .spool
                .read_to_end(&mut stdout)
                .context(SuperviseSnafu {
                    operation: "read captured workflow output",
                })?;
            stdout
        };
        self.receiver.finish();
        let outcome = LocalOutcome {
            status,
            interrupted: self.interrupted_at.is_some(),
            stdout,
            stderr: capture.stderr_display(),
            capture_error: capture.error.clone(),
            capture_failure: capture.failure.clone(),
            call_events: self.receiver.reader().call_events(),
        };
        self.child.running = false;
        *self
            .completion
            .lock()
            .expect("run completion was not poisoned") = Some((status, at));
        self.finished = true;
        Ok(Some(outcome))
    }
}

pub fn terminal_area() -> io::Result<Rect> {
    let size = rustix::termios::tcgetwinsize(io::stderr()).map_err(io::Error::from)?;
    if size.ws_col == 0 || size.ws_row == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "terminal has no usable size",
        ));
    }
    Ok(Rect::new(0, 0, size.ws_col, size.ws_row))
}

fn report(message: &str) {
    let _ = writeln!(io::stderr().lock(), "{message}");
}

fn exit_code(status: ExitStatus) -> u8 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1)
}

pub struct SignalGuard {
    pub interrupted: Arc<AtomicBool>,
    id: SigId,
}

impl SignalGuard {
    pub fn register() -> io::Result<Self> {
        let interrupted = Arc::new(AtomicBool::new(false));
        let id = flag::register(SIGINT, Arc::clone(&interrupted))?;
        Ok(Self { interrupted, id })
    }
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        unregister(self.id);
    }
}

pub struct TerminalGuard;

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let guard = Self;
        execute!(io::stderr(), EnterAlternateScreen, Hide)?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stderr(), Show, LeaveAlternateScreen);
    }
}

struct ChildGuard {
    child: Box<dyn ChildWrapper>,
    running: bool,
}

impl ChildGuard {
    fn new(child: Box<dyn ChildWrapper>) -> Self {
        Self {
            child,
            running: true,
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.running {
            let _ = self.child.start_kill();
        }
        let _ = self.child.wait();
    }
}

struct Capture {
    stdout: Option<std::process::ChildStdout>,
    stderr: Option<std::process::ChildStderr>,
    spool: File,
    written: u64,
    limit: u64,
    stderr_tail: VecDeque<u8>,
    stderr_dropped: u64,
    error: Option<String>,
    failure: Option<Arc<CaptureFailure>>,
    record_mode: bool,
    record_bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct CaptureView {
    pub stdout_open: bool,
    pub stderr_open: bool,
    pub stderr_tail: String,
    pub stderr_dropped: u64,
    pub error: Option<String>,
}

struct CaptureWorker {
    capture: Arc<Mutex<Capture>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    cancellation: Option<mf_runtime::StreamCancellation>,
    record_processing: Arc<AtomicBool>,
    record_mode: bool,
    progress: Arc<Mutex<Instant>>,
}

impl CaptureWorker {
    fn start(capture: Arc<Mutex<Capture>>) -> Self {
        Self::start_with_stream(capture, None)
    }

    fn start_with_stream(
        capture: Arc<Mutex<Capture>>,
        mut stream: Option<StreamCaptureState>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let processing = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(Instant::now()));
        let cancellation = stream.as_ref().map(|stream| stream.cancellation.clone());
        let record_mode = stream.is_some();
        let shared_capture = Arc::clone(&capture);
        let shared_stop = Arc::clone(&stop);
        let shared_processing = Arc::clone(&processing);
        let shared_progress = Arc::clone(&progress);
        let worker = thread::spawn(move || {
            let mut codec_done = stream.is_none();
            while !shared_stop.load(Ordering::Relaxed) {
                let (mut bytes, open, stdout_closed, process_records) = {
                    let mut capture = shared_capture
                        .lock()
                        .expect("capture lock was not poisoned");
                    capture.drain();
                    let bytes = std::mem::take(&mut capture.record_bytes);
                    let stdout_closed = capture.stdout.is_none();
                    let process_records = !codec_done && (!bytes.is_empty() || stdout_closed);
                    if process_records {
                        shared_processing.store(true, Ordering::Release);
                    }
                    (
                        bytes,
                        capture.stdout.is_some() || capture.stderr.is_some(),
                        stdout_closed,
                        process_records,
                    )
                };
                if process_records {
                    let stream = stream.as_mut().expect("stream codec exists");
                    let cancellation = stream.cancellation.clone();
                    let mut emit = |mut values| -> Result<(), CaptureFailure> {
                        loop {
                            ensure!(!shared_stop.load(Ordering::Relaxed), OutputStoppedSnafu);
                            if let Some(error) = cancellation.failure() {
                                return Err(error).context(OutputCancelledSnafu);
                            }
                            match stream.sender.try_send(values) {
                                Ok(()) => {
                                    *shared_progress.lock().unwrap() = Instant::now();
                                    return Ok(());
                                }
                                Err(std::sync::mpsc::TrySendError::Full(pending)) => {
                                    values = pending
                                }
                                Err(error) => return Err(error).context(OutputDeliverySnafu),
                            }
                            thread::sleep(Duration::from_millis(10));
                        }
                    };
                    let mut result = stream.decoder.feed(&bytes, &mut emit);
                    if result.is_ok() && stdout_closed {
                        result = stream.decoder.finish(&mut emit);
                        codec_done = true;
                    }
                    if let Err(error) = result {
                        codec_done = true;
                        shared_capture.lock().unwrap().record_failure(error);
                    }
                    shared_processing.store(false, Ordering::Release);
                }
                if !bytes.is_empty() {
                    *shared_progress.lock().unwrap() = Instant::now();
                    bytes.clear();
                    shared_capture
                        .lock()
                        .expect("capture lock was not poisoned")
                        .record_bytes = bytes;
                }
                if !open {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
        });
        Self {
            capture,
            stop,
            worker: Some(worker),
            cancellation,
            record_processing: processing,
            record_mode,
            progress,
        }
    }

    fn cancel(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn cancelled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(|cancellation| cancellation.failure().is_some())
    }

    fn processing(&self) -> bool {
        self.record_processing.load(Ordering::Acquire)
    }

    fn records_pending(&self) -> bool {
        self.record_mode
            && (self.processing() || self.progress.lock().unwrap().elapsed() < DRAIN_TIMEOUT)
    }

    fn view(&self) -> CaptureView {
        self.capture
            .lock()
            .expect("capture lock was not poisoned")
            .view()
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for CaptureWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Capture {
    fn record_failure(&mut self, error: CaptureFailure) {
        self.error.get_or_insert_with(|| error.to_string());
        self.failure.get_or_insert_with(|| Arc::new(error));
    }
    fn view(&self) -> CaptureView {
        CaptureView {
            stdout_open: self.stdout.is_some(),
            stderr_open: self.stderr.is_some(),
            stderr_tail: self.stderr_display(),
            stderr_dropped: self.stderr_dropped,
            error: self.error.clone(),
        }
    }

    fn new() -> io::Result<Self> {
        Ok(Self {
            stdout: None,
            stderr: None,
            spool: tempfile::tempfile()?,
            written: 0,
            limit: MAX_STDOUT_BYTES,
            stderr_tail: VecDeque::new(),
            stderr_dropped: 0,
            error: None,
            failure: None,
            record_mode: false,
            record_bytes: Vec::new(),
        })
    }

    fn attach(&mut self, child: &mut Box<dyn ChildWrapper>) -> io::Result<()> {
        let stdout = child.stdout().take().expect("stdout was piped");
        let stderr = child.stderr().take().expect("stderr was piped");
        set_nonblocking(&stdout)?;
        set_nonblocking(&stderr)?;
        self.stdout = Some(stdout);
        self.stderr = Some(stderr);
        Ok(())
    }

    fn drain(&mut self) {
        let mut bytes = [0u8; 8192];
        for _ in 0..8 {
            let Some(stdout) = self.stdout.as_mut() else {
                break;
            };
            match stdout.read(&mut bytes) {
                Ok(0) => {
                    self.stdout = None;
                    break;
                }
                Ok(len) => self.store_stdout(&bytes[..len]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.record_failure(IoSnafu { operation: "read" }.into_error(error));
                    self.stdout = None;
                    break;
                }
            }
        }
        for _ in 0..8 {
            let Some(stderr) = self.stderr.as_mut() else {
                break;
            };
            match stderr.read(&mut bytes) {
                Ok(0) => {
                    self.stderr = None;
                    break;
                }
                Ok(len) => self.store_stderr(&bytes[..len]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.store_stderr(format!("\nstderr read failed: {error}").as_bytes());
                    self.stderr = None;
                    break;
                }
            }
        }
    }

    fn store_stdout(&mut self, bytes: &[u8]) {
        if self.error.is_some() {
            return;
        }
        if self.record_mode {
            self.record_bytes.extend_from_slice(bytes);
            return;
        }
        let allowed = (self.limit - self.written).min(bytes.len() as u64) as usize;
        if let Err(error) = self.spool.write_all(&bytes[..allowed]) {
            self.record_failure(
                IoSnafu {
                    operation: "spool write",
                }
                .into_error(error),
            );
            return;
        }
        self.written += allowed as u64;
        if allowed < bytes.len() {
            self.record_failure(SpoolLimitSnafu { limit: self.limit }.build());
        }
    }

    fn store_stderr(&mut self, bytes: &[u8]) {
        let excess = self
            .stderr_tail
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(MAX_STDERR_BYTES);
        for _ in 0..excess.min(self.stderr_tail.len()) {
            self.stderr_tail.pop_front();
        }
        self.stderr_dropped = self.stderr_dropped.saturating_add(excess as u64);
        self.stderr_tail.extend(
            bytes[bytes.len().saturating_sub(MAX_STDERR_BYTES)..]
                .iter()
                .copied(),
        );
    }

    fn stderr_display(&self) -> String {
        let bytes: Vec<_> = self.stderr_tail.iter().rev().take(2048).copied().collect();
        let text =
            String::from_utf8_lossy(&bytes.into_iter().rev().collect::<Vec<_>>()).into_owned();
        text.chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect()
    }
}

fn set_nonblocking(fd: impl AsFd) -> io::Result<()> {
    let flags = OFlag::from_bits_truncate(fcntl(&fd, FcntlArg::F_GETFL)?);
    fcntl(&fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
    Ok(())
}

#[derive(Default)]
struct ViewState {
    graph_notice: Option<String>,
    latest_node_sequence: Option<mf_telemetry::Count>,
    reveal_selected: bool,
    graph_size: Option<(u16, u16)>,
    details_visible: bool,
    help_visible: bool,
    traces: TraceView,
    path: Vec<String>,
    offset: (u32, u32),
    selected: usize,
    pass_offset: usize,
    details_offset: usize,
    details_height: usize,
}

impl ViewState {
    fn follow_latest(&mut self, snapshot: &StateSnapshot, layout: &GraphLayout) {
        let Some(latest) = &snapshot.latest_node_event else {
            return;
        };
        if self
            .latest_node_sequence
            .is_some_and(|sequence| sequence >= latest.sequence)
        {
            return;
        }
        self.latest_node_sequence = Some(latest.sequence);
        if self.pass_offset != 0 || self.traces.visible {
            return;
        }
        let path = &latest.node.path;
        if !self
            .path
            .iter()
            .zip(path)
            .all(|(id, entry)| *id == entry.loop_id)
            || self.path.len() > path.len()
        {
            return;
        }
        // Keep the inspected scope; descendants focus their visible enclosing Loop.
        let id = path
            .get(self.path.len())
            .map_or(&latest.node.id, |entry| &entry.loop_id);
        if let Some(selected) = layout.display_index_for_node_id(id) {
            if self.selected != selected {
                self.details_offset = 0;
            }
            self.selected = selected;
            self.reveal_selected = true;
        }
    }

    fn reveal(&mut self, layout: &GraphLayout, area: Rect) {
        let size = (area.width, area.height);
        if (self.reveal_selected || self.graph_size != Some(size))
            && area.width > 0
            && area.height > 0
        {
            if let Some(node) = layout.nodes().get(self.selected) {
                self.offset = scroll_into_view(self.offset, node.rect, size);
            }
            self.reveal_selected = false;
        }
        self.graph_size = Some(size);
    }

    fn handle_help(&mut self, key: KeyCode) -> bool {
        if key == KeyCode::Char('?') {
            self.help_visible = !self.help_visible;
            return true;
        }
        if self.help_visible {
            if matches!(key, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                self.help_visible = false;
            }
            return true;
        }
        false
    }

    fn layout<'a>(
        &self,
        root: &'a GraphLayout,
        bodies: &'a BTreeMap<Vec<String>, GraphLayout>,
    ) -> &'a GraphLayout {
        if self.path.is_empty() {
            root
        } else {
            &bodies[&self.path]
        }
    }

    fn pass<'a>(&self, snapshot: &'a StateSnapshot) -> Option<&'a LoopPassObservation> {
        let matching: Vec<_> = snapshot
            .loop_passes
            .iter()
            .filter(|pass| {
                pass.path
                    .iter()
                    .map(|entry| &entry.loop_id)
                    .eq(self.path.iter())
            })
            .collect();
        let offset = self.pass_offset.min(matching.len().checked_sub(1)?);
        matching.get(matching.len() - offset - 1).copied()
    }

    pub fn handle_key(
        &mut self,
        key: KeyCode,
        snapshot: &StateSnapshot,
        root: &GraphLayout,
        bodies: &BTreeMap<Vec<String>, GraphLayout>,
        traces: &TraceSnapshot,
        logs: &LogSnapshot,
    ) {
        if self.traces.handle_key(key, traces, logs) {
            return;
        }
        match key {
            KeyCode::Char('d') => self.details_visible = !self.details_visible,
            KeyCode::Esc if self.details_visible => self.details_visible = false,
            KeyCode::PageDown if self.details_visible => {
                self.details_offset = self
                    .details_offset
                    .saturating_add(self.details_height.max(1))
            }
            KeyCode::PageUp if self.details_visible => {
                self.details_offset = self
                    .details_offset
                    .saturating_sub(self.details_height.max(1))
            }
            KeyCode::Char('l') => {
                if let Some(node) = self.layout(root, bodies).nodes().get(self.selected)
                    && matches!(node.kind.as_str(), "workflow.loop" | "builtin.iteration")
                {
                    let mut child = self.path.clone();
                    child.push(node.id.clone());
                    if bodies.contains_key(&child) {
                        self.path = child;
                        self.offset = (0, 0);
                        self.selected = 0;
                        self.pass_offset = 0;
                        self.details_offset = 0;
                    }
                }
            }
            KeyCode::Char('h') | KeyCode::Esc => {
                if !self.path.is_empty() {
                    self.path.pop();
                    self.offset = (0, 0);
                    self.selected = 0;
                    self.pass_offset = 0;
                    self.details_offset = 0;
                }
            }
            KeyCode::Char('[') => {
                let count = snapshot
                    .loop_passes
                    .iter()
                    .filter(|pass| {
                        pass.path
                            .iter()
                            .map(|entry| &entry.loop_id)
                            .eq(self.path.iter())
                    })
                    .count();
                self.pass_offset = (self.pass_offset + 1).min(count.saturating_sub(1));
            }
            KeyCode::Char(']') => self.pass_offset = self.pass_offset.saturating_sub(1),
            _ => {
                let nodes = self.layout(root, bodies).nodes().len();
                let previous = self.selected;
                navigate(key, &mut self.offset, &mut self.selected, nodes);
                if matches!(
                    key,
                    KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('j' | 'k')
                ) {
                    self.reveal_selected = true;
                }
                if previous != self.selected {
                    self.details_offset = 0;
                }
            }
        }
    }
}

fn supervise(
    run: &mut LocalRun,
    view: &mut ObservationView,
    terminal: &mut Terminal<CrosstermBackend<Box<dyn Write>>>,
    signal: &SignalGuard,
) -> Result<LocalOutcome, RunError> {
    let mut outcome = None;
    let observation = run.observation();
    loop {
        if outcome.is_none() {
            outcome = run.poll()?;
        }
        if signal.interrupted.swap(false, Ordering::Relaxed) {
            if let Some(outcome) = outcome {
                return Ok(outcome);
            }
            run.cancel();
        }
        view.draw(terminal, &observation)?;
        if event::poll(Duration::from_millis(100)).context(TerminalSnafu)? {
            match event::read().context(TerminalSnafu)? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    let quit = (!view.state.help_visible
                        && (matches!(key.code, KeyCode::Char('q') | KeyCode::Enter)
                            || (key.code == KeyCode::Esc
                                && view.state.path.is_empty()
                                && !view.state.traces.visible
                                && !view.state.details_visible)))
                        || (key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL));
                    if quit && let Some(outcome) = outcome.take() {
                        return Ok(outcome);
                    }
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        run.cancel();
                    } else {
                        view.handle_key(key.code, &observation);
                    }
                }
                Event::Resize(width, height) => {
                    terminal
                        .resize(Rect::new(0, 0, width, height))
                        .context(TerminalSnafu)?;
                }
                _ => {}
            }
        }
    }
}

fn navigate(key: KeyCode, offset: &mut (u32, u32), selected: &mut usize, nodes: usize) {
    match key {
        KeyCode::Left => offset.0 = offset.0.saturating_sub(4),
        KeyCode::Right => offset.0 = offset.0.saturating_add(4),
        KeyCode::Up => offset.1 = offset.1.saturating_sub(2),
        KeyCode::Down => offset.1 = offset.1.saturating_add(2),
        KeyCode::Char('f') => *offset = (0, 0),
        KeyCode::Tab | KeyCode::Char('j') => *selected = (*selected + 1) % nodes.max(1),
        KeyCode::BackTab | KeyCode::Char('k') => {
            *selected = (*selected + nodes.max(1) - 1) % nodes.max(1)
        }
        _ => {}
    }
}

fn scroll_into_view(offset: (u32, u32), node: GraphRect, size: (u16, u16)) -> (u32, u32) {
    fn axis(offset: u32, start: u32, length: u32, visible: u16) -> u32 {
        let visible = u32::from(visible);
        if visible == 0 {
            offset
        } else if start < offset || length > visible {
            start
        } else {
            offset.max(start.saturating_add(length).saturating_sub(visible))
        }
    }
    (
        axis(offset.0, node.x, node.width, size.0),
        axis(offset.1, node.y, node.height, size.1),
    )
}

#[allow(clippy::too_many_arguments)]
fn draw(
    terminal: &mut Terminal<CrosstermBackend<Box<dyn Write>>>,
    root_layout: &GraphLayout,
    body_layouts: &BTreeMap<Vec<String>, GraphLayout>,
    description: &WorkflowDescription,
    snapshot: &StateSnapshot,
    capture: &CaptureView,
    elapsed: Duration,
    view_state: &mut ViewState,
    receiver: &ObservationReader,
    status: Option<ExitStatus>,
) -> Result<(), RunError> {
    if view_state.traces.visible {
        terminal
            .draw(|frame| {
                let traces = receiver.trace_snapshot();
                let logs = receiver.log_snapshot();
                view_state.traces.draw(
                    frame,
                    &TraceViewSnapshot {
                        traces: &traces,
                        logs: &logs,
                        data: &receiver.data_snapshot(),
                        spans: snapshot.traces,
                        finished: status.is_some(),
                    },
                );
                draw_help(frame, view_state);
            })
            .context(TerminalSnafu)?;
        return Ok(());
    }
    let layout = view_state.layout(root_layout, body_layouts);
    view_state.follow_latest(snapshot, layout);
    let pass = view_state.pass(snapshot);
    let static_body = description.body(&view_state.path).is_some_and(|body| {
        body.nodes
            .iter()
            .any(|node| node.kind == mf_runtime::ITERATION_INPUT_KIND)
    });
    let display_nodes =
        (!view_state.path.is_empty()).then(|| body_observations(layout, pass, static_body));
    terminal.draw(|frame| {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1), Constraint::Min(1), Constraint::Length(2),
        ]).areas(frame.area());
        let phase = status.map_or_else(|| "Running".to_owned(), |status| format!("Exited: {}", exit_code(status)));
        let workflow = snapshot.workflow_outcome.map_or("Unknown", |outcome| match outcome {
            mf_telemetry::event::Outcome::Succeeded => "Succeeded",
            mf_telemetry::event::Outcome::Failed => "Failed",
        });
        let scope = if view_state.path.is_empty() {
            "root".to_owned()
        } else {
            format!("{:?}", view_state.path)
        };
        let pass_label = if static_body { "  |  Static body; item execution in spans".into() } else { pass.map_or(String::new(), |pass| {
            let index = pass.path.last().map_or(0, |entry| entry.index.get());
            match pass.stream_invocation {
                Some(owner) => format!("  |  Invocation: {owner}  |  Pass: {index}"),
                None => format!("  |  Pass: {index}"),
            }
        }) };
        frame.render_widget(Paragraph::new(format!(
            "{}  |  Workflow: {workflow}  |  Process: {phase}  |  Scope: {scope}{pass_label}  |  {:.1}s",
            connection_summary(layout), elapsed.as_secs_f64()
        )), header);
        let (graph, details) = if view_state.details_visible {
            let [graph, details] = Layout::horizontal([Constraint::Percentage(70), Constraint::Percentage(30)]).areas(body);
            (graph, Some(details))
        } else {
            (body, None)
        };
        let graph = if view_state.graph_notice.is_some() || layout.unresolved_count() > 0 {
            let [warning, graph] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(graph);
            frame.render_widget(Paragraph::new(view_state.graph_notice.clone().unwrap_or_else(|| routing_warning(layout))).style(Style::default().fg(Color::Yellow)), warning);
            graph
        } else { graph };
        view_state.reveal(layout, graph);
        let mut graph_view = GraphView::new(layout)
            .reader(receiver)
            .selected(view_state.selected)
            .offset(view_state.offset.0, view_state.offset.1)
            .elapsed_ns(elapsed.as_nanos().min(u64::MAX as u128) as u64);
        if view_state.path.is_empty() {
            graph_view = graph_view.snapshot(snapshot);
        } else if let Some(nodes) = &display_nodes {
            graph_view = graph_view.nodes(nodes);
        }
        frame.render_widget(graph_view, graph);
        if let Some(details) = details {
            view_state.details_height = usize::from(details.height.saturating_sub(2));
            let observations = display_nodes.as_deref().unwrap_or(&snapshot.nodes);
            let selected = view_state.selected;
            let collapsed_cluster = layout.is_collapsed_cluster(selected);
            let selected_node = (!collapsed_cluster)
                .then(|| {
                    layout
                        .sources(selected)
                        .first()
                        .and_then(|source| observations.get(*source))
                })
                .flatten();
            let cluster = layout.cluster_of(selected).map(|cluster| {
                let cluster = &layout.clusters()[cluster];
                if collapsed_cluster {
                    collapsed_cluster_details(cluster, layout.sources(selected), observations)
                } else {
                    format!(
                        "Cluster: {}\n{}\nMembers: {}\n",
                        display_text(&cluster.label),
                        display_text(&cluster.annotation),
                        cluster.members.len()
                    )
                }
            });
            let mut bindings = binding_details(description, &view_state.path, selected_node);
            if let Some(node) = selected_node.filter(|node| node.kind == "builtin.llm") {
                bindings.push_str(&crate::performance::details(&crate::performance::node_calls(receiver, node)));
            }
            let details_body = format!(
                "{}\n{}\n{}\n{}",
                view_state.graph_notice.as_deref().unwrap_or_default(),
                cluster.unwrap_or_default(),
                details_text(selected_node, pass, snapshot, capture, &bindings),
                layout.routing_details(selected)
            );
            let (window, offset) = details_window(&details_body, usize::from(details.width.saturating_sub(2)), usize::from(details.height.saturating_sub(2)), view_state.details_offset);
            view_state.details_offset = offset;
            frame.render_widget(Paragraph::new(window)
                .block(Block::default().title("Details · d hide").title_bottom("PgUp/PgDn scroll").borders(Borders::ALL)), details);
        }
        let lifecycle = &snapshot.lifecycle;
        let controls = if status.is_some() { "q/Enter close" } else { "Ctrl-C interrupt" };
        frame.render_widget(Paragraph::new(format!(
            "Observation: {:?} | gaps: {} | drops: {} | errors: {} | traces: {}\nArrows pan | Tab select | d details | ? help | {controls}",
            lifecycle.completeness, lifecycle.known_missing_count, lifecycle.local_drops,
            lifecycle.observation_errors + lifecycle.protocol_conflicts, snapshot.traces.observed_spans,
        )), footer);
        draw_help(frame, view_state);
    }).context(TerminalSnafu)?;
    Ok(())
}

fn draw_help(frame: &mut Frame<'_>, state: &ViewState) {
    if !state.help_visible {
        return;
    }
    let text = if state.traces.visible {
        "Up/Down, j/k   Select entry\nTab            Switch pane\n\
         g              Spans / session logs\nl              Focus related records\n\
         Home           First entry\nEnd, f         Follow latest entry\n\
         PgUp/PgDn      Scroll / page records\nv, Esc         Return to graph\n\
         Ctrl-C         Interrupt / close"
    } else {
        "Arrows         Pan graph\nTab / Shift-Tab, j/k   Select node\n\
         d              Show / hide details\nPgUp/PgDn      Scroll visible details\n\
         e              Cycle connection view\nc              Collapse / expand cluster\nv              Open OTEL tracing\n\
         l              Enter selected body\nh              Return to parent scope\n\
         Esc            Close details / return to parent\n\
         [ / ]          Older / newer pass\nf              Reset viewport\n\
         Ctrl-C         Interrupt / close\nq, Enter       Close after completion"
    };
    let [_, horizontal, _] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(58),
        Constraint::Fill(1),
    ])
    .areas(frame.area());
    let [_, popup, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(text.lines().count() as u16 + 2),
        Constraint::Fill(1),
    ])
    .areas(horizontal);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(text).block(
            Block::default()
                .title("Shortcuts")
                .title_bottom("? / Esc close")
                .borders(Borders::ALL),
        ),
        popup,
    );
}

fn body_observations(
    layout: &GraphLayout,
    pass: Option<&LoopPassObservation>,
    static_body: bool,
) -> Vec<NodeObservation> {
    let order = || {
        layout
            .all_nodes()
            .iter()
            .map(|node| (node.id.as_str(), node.kind.as_str()))
    };
    if !static_body && let Some(pass) = pass {
        return pass.display_nodes(order());
    }
    order()
        .map(|(id, kind)| {
            let mut node = NodeObservation::pending(id, kind);
            if static_body {
                node.status = crate::state::NodeStatus::Unknown;
            }
            node
        })
        .collect()
}

fn connection_summary(layout: &GraphLayout) -> String {
    let label = layout.connection_view().label();
    let hidden = layout.hidden_data_count();
    let mut summary = if hidden > 0 {
        format!("{label}: {hidden} data mappings hidden")
    } else {
        format!("Connections: {label}")
    };
    let transitive = layout.hidden_transitive_count();
    if transitive > 0 {
        summary.push_str(&format!(" | {transitive} transitive dependencies hidden"));
    }
    let collapsed = layout
        .clusters()
        .iter()
        .filter(|cluster| cluster.collapsed)
        .count();
    if collapsed > 0 {
        summary.push_str(&format!(
            " | {collapsed}/{} clusters collapsed | {} internal edges hidden",
            layout.clusters().len(),
            layout.hidden_cluster_edge_count()
        ));
    }
    summary
}

fn display_text(value: &str) -> String {
    let mut text = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\t' => text.push_str("    "),
            ch if ch.is_control() && ch != '\n' => text.push(' '),
            ch => text.push(ch),
        }
    }
    text
}

fn collapsed_cluster_details(
    cluster: &GraphCluster,
    members: &[usize],
    observations: &[NodeObservation],
) -> String {
    let mut text = format!(
        "Cluster: {}\n{}\nMembers:\n",
        display_text(&cluster.label),
        display_text(&cluster.annotation)
    );
    for member in members {
        let (id, status) = observations.get(*member).map_or_else(
            || ("?", crate::state::NodeStatus::Pending),
            |observation| (observation.id.as_str(), observation.status),
        );
        text.push_str(&format!("  {id}: {status:?}\n"));
    }
    text
}

fn binding_details(
    description: &WorkflowDescription,
    path: &[String],
    node: Option<&NodeObservation>,
) -> String {
    let Some(node) = node else {
        return String::new();
    };
    let (data, controls, orders) = if path.is_empty() {
        (
            &description.data_edges,
            &description.control_edges,
            &description.order_edges,
        )
    } else if let Some(body) = description.body(path) {
        (&body.data_edges, &body.control_edges, &body.order_edges)
    } else {
        return String::new();
    };
    let label = |value: &str| value.escape_debug().to_string();
    let mut inputs: BTreeMap<(&str, bool), Vec<_>> = BTreeMap::new();
    for edge in data.iter().filter(|edge| edge.to_node == node.id) {
        inputs
            .entry((&edge.to_input, edge.exclusive))
            .or_default()
            .push(edge);
    }
    let mut text = String::from("Input bindings:\n");
    if inputs.is_empty() {
        text.push_str("  none\n");
    }
    for ((input, exclusive), mut sources) in inputs {
        sources.sort_by_key(|edge| (&edge.from_node, &edge.from_output));
        text.push_str(&format!(
            "  {}{}\n",
            label(input),
            if exclusive { " [exclusive]" } else { "" }
        ));
        let all_skipped = !node.possibly_missing_events
            && !node.conflicted
            && sources.iter().all(|edge| {
                node.skip_causes.iter().any(|cause| {
                    cause.source_node() == edge.from_node
                        && cause.source_output() == Some(edge.from_output.as_str())
                })
            });
        for edge in sources {
            text.push_str(&format!(
                "    {}.{}\n",
                label(&edge.from_node),
                label(&edge.from_output)
            ));
        }
        if exclusive {
            let selected = (!node.possibly_missing_events && !node.conflicted)
                .then(|| {
                    node.input_selections
                        .iter()
                        .find(|selection| selection.input == input)
                })
                .flatten();
            if let Some(selected) = selected {
                text.push_str(&format!(
                    "    selected: {}.{}\n",
                    label(&selected.source_node),
                    label(&selected.source_output)
                ));
            } else if all_skipped {
                text.push_str("    selected: none (all candidates skipped)\n");
            } else {
                text.push_str("    selected: unknown\n");
            }
        }
    }
    let mut prerequisites = Vec::new();
    for edge in controls.iter().filter(|edge| edge.to_node == node.id) {
        prerequisites.push(format!(
            "  output {}.{}",
            label(&edge.from_node),
            label(&edge.from_output)
        ));
    }
    for edge in orders.iter().filter(|edge| edge.to_node == node.id) {
        prerequisites.push(format!("  success of {}", label(&edge.from_node)));
    }
    prerequisites.sort();
    text.push_str("Execution prerequisites:\n");
    if prerequisites.is_empty() {
        text.push_str("  none\n");
    } else {
        text.push_str(&prerequisites.join("\n"));
        text.push('\n');
    }
    let mut outgoing = Vec::new();
    for edge in data.iter().filter(|edge| edge.from_node == node.id) {
        outgoing.push(format!(
            "  {} -> {}.{}{}",
            label(&edge.from_output),
            label(&edge.to_node),
            label(&edge.to_input),
            if edge.exclusive { " [exclusive]" } else { "" }
        ));
    }
    for edge in controls.iter().filter(|edge| edge.from_node == node.id) {
        outgoing.push(format!(
            "  output {} -> {}",
            label(&edge.from_output),
            label(&edge.to_node)
        ));
    }
    for edge in orders.iter().filter(|edge| edge.from_node == node.id) {
        outgoing.push(format!("  success -> {}", label(&edge.to_node)));
    }
    outgoing.sort();
    text.push_str("Consumers:\n");
    if outgoing.is_empty() {
        text.push_str("  none\n");
    } else {
        text.push_str(&outgoing.join("\n"));
        text.push('\n');
    }
    if node.omitted_input_selections > 0 {
        text.push_str(&format!(
            "Omitted selection observations: {}\n",
            node.omitted_input_selections
        ));
    }
    text
}

fn details_window(text: &str, width: usize, height: usize, offset: usize) -> (String, usize) {
    if width == 0 || height == 0 {
        return (String::new(), 0);
    }
    let mut tail = std::collections::VecDeque::new();
    let mut window = Vec::new();
    let mut rows = 0;
    let mut emit = |line: String| {
        if rows >= offset && window.len() < height {
            window.push(line.clone());
        }
        if tail.len() == height {
            tail.pop_front();
        }
        tail.push_back(line);
        rows += 1;
    };
    for source in text.split('\n') {
        let span = ratatui::text::Span::raw(source);
        let mut line = String::new();
        let mut used = 0;
        for grapheme in span.styled_graphemes(Style::default()) {
            let cells = ratatui::text::Span::raw(grapheme.symbol).width();
            if cells == 0 {
                continue;
            }
            if used + cells > width && !line.is_empty() {
                emit(std::mem::take(&mut line));
                used = 0;
            }
            if cells > width {
                line.push('?');
                used += 1;
            } else {
                line.push_str(grapheme.symbol);
                used += cells;
            }
        }
        emit(line);
    }
    let clamped = offset.min(rows.saturating_sub(height));
    if clamped != offset {
        (tail.into_iter().collect::<Vec<_>>().join("\n"), clamped)
    } else {
        (window.join("\n"), offset)
    }
}

fn routing_warning(layout: &GraphLayout) -> String {
    let count = layout.unresolved_count();
    if count == 0 {
        String::new()
    } else {
        format!("Connections: {count} unresolved (see Details)")
    }
}

fn details_text(
    node: Option<&NodeObservation>,
    pass: Option<&LoopPassObservation>,
    snapshot: &StateSnapshot,
    capture: &CaptureView,
    bindings: &str,
) -> String {
    let mut details = String::new();
    let parent_path = pass.map_or(&[][..], |pass| pass.path.as_slice());
    if let Some(node) = node {
        details.push_str(&format!(
            "{} ({})\nStatus: {:?}\n",
            node.id, node.kind, node.status
        ));
        details.push_str(bindings);
        if let Some(stream) = &snapshot.stream {
            if parent_path.is_empty()
                && let Some(metrics) = stream.nodes.iter().find(|metrics| metrics.id == node.id)
            {
                details.push_str(&format!(
                    "Observed calls: {}\nCompleted: {} | Results: {}\n",
                    metrics.observed_invocations,
                    metrics.observed_completions,
                    metrics.observed_results
                ));
                if let Some(count) = metrics.buffered_items {
                    details.push_str(&format!("Buffered items: {}\n", count.get()));
                }
                if let Some(reason) = &metrics.last_flush_reason {
                    details.push_str(&format!("Last flush: {reason}\n"));
                }
            }
            if let Some(invocation) = stream.invocations.iter().rev().find(|invocation| {
                invocation.node.id == node.id
                    && invocation.path == parent_path
                    && invocation.identity.parent == pass.and_then(|pass| pass.stream_invocation)
            }) {
                details.push_str(&format!(
                    "Invocation: {} ({:?})\n",
                    invocation.identity.invocation, invocation.identity.trigger
                ));
                if let Some(message) = invocation.identity.message {
                    details.push_str(&format!(
                        "Message: {}:{}\n",
                        message.domain, message.sequence
                    ));
                }
                if node.kind == "workflow.loop" {
                    details.push_str(&format!(
                        "Completed passes: {}\n",
                        invocation.completed_passes
                    ));
                    if let Some(index) = invocation.active_pass {
                        details.push_str(&format!("Active pass: {}\n", index.get()));
                    }
                    if let Some(summary) = &invocation.loop_summary {
                        details.push_str(&format!("Stop reason: {:?}\n", summary.reason));
                    }
                }
            }
            if pass.is_some_and(|pass| !pass.nodes.iter().any(|detail| detail.id == node.id)) {
                details.push_str("Node detail evicted; compact status retained\n");
            }
        }
        if let Some(duration) = node.duration_ns {
            details.push_str(&format!(
                "Duration: {}\n",
                format_duration_ns(duration.get().max(0) as u64)
            ));
        }
        if !node.produced_ports.is_empty() {
            details.push_str(&format!("Produced: {}\n", node.produced_ports.join(", ")));
        }
        if !node.skipped_ports.is_empty() {
            details.push_str(&format!("Skipped: {}\n", node.skipped_ports.join(", ")));
        }
        for cause in &node.skip_causes {
            match cause.source_output() {
                Some(port) => {
                    details.push_str(&format!("Skipped by {}.{port}\n", cause.source_node()))
                }
                None => details.push_str(&format!(
                    "Skipped by unsuccessful node {}\n",
                    cause.source_node()
                )),
            }
        }
        if node.omitted_port_names != 0 || node.omitted_skip_causes != 0 {
            details.push_str(&format!(
                "Omitted ports: {}, skip causes: {}\n",
                node.omitted_port_names, node.omitted_skip_causes
            ));
        }
        if let Some(failure) = &node.failure {
            details.push_str(&format!(
                "Failure ({:?}): {}\n",
                failure.phase, failure.message
            ));
        }
        if node.possibly_missing_events || node.conflicted {
            details.push_str("Node observation may be incomplete\n");
        }
        if node.kind == "workflow.loop"
            && snapshot.stream.as_ref().is_none_or(|stream| {
                parent_path.is_empty()
                    && !stream.invocations.iter().any(|invocation| {
                        invocation.node.id == node.id && invocation.path.is_empty()
                    })
            })
            && let Some(loop_state) = snapshot
                .loop_overviews
                .iter()
                .find(|overview| overview.parent_path == parent_path && overview.loop_id == node.id)
        {
            details.push_str(&format!(
                "Completed passes: {}\n",
                loop_state.completed_passes
            ));
            if let Some(index) = loop_state.active_index {
                details.push_str(&format!("Active pass: {}\n", index.get()));
            }
            if let Some(reason) = loop_state.stop_reason {
                details.push_str(&format!("Stop reason: {reason:?}\n"));
            }
        }
    }
    if let Some(stream) = &snapshot.stream {
        if let Some(counts) = &stream.counts {
            details.push_str(&format!("Workflow totals\nStartup: {} | Messages: {}\nCompleted frames: {}\nDelivered outputs: {}\n", counts.startup_frames, counts.emitted_messages, counts.completed_frames, counts.delivered_outputs));
        }
        if let Some(failure) = &stream.failure {
            details.push_str(&format!(
                "Workflow failure ({}): {}\n",
                failure.phase, failure.message
            ));
        }
        if stream.hidden_invocations != 0 {
            details.push_str(&format!(
                "Older invocation details hidden: {}\n",
                stream.hidden_invocations
            ));
        }
        if stream.unverified_retransmissions != 0 {
            details.push_str(&format!(
                "Retransmissions outside verification coverage: {}\n",
                stream.unverified_retransmissions
            ));
        }
        let unresolved = stream
            .invocations
            .iter()
            .filter(|invocation| invocation.node.possibly_missing_events)
            .count();
        if unresolved != 0 {
            details.push_str(&format!("Unresolved invocation outcomes: {unresolved}\n"));
        }
    }
    if snapshot.hidden_loop_passes != 0 {
        details.push_str(&format!(
            "Older pass details hidden: {}\n",
            snapshot.hidden_loop_passes
        ));
    }
    if snapshot.lifecycle.final_sequence.is_none() {
        details.push_str("Final lifecycle boundary missing\n");
    }
    if let Some(failure) = &snapshot.workflow_failure {
        details.push_str(&format!(
            "Workflow failure ({:?}): {}\n",
            failure.phase, failure.message
        ));
    }
    if snapshot.traces.local_drops != 0 {
        details.push_str(&format!("Trace drops: {}\n", snapshot.traces.local_drops));
    }
    if snapshot.diagnostic_bytes_dropped != 0 {
        details.push_str(&format!(
            "Observation diagnostics dropped {} bytes\n",
            snapshot.diagnostic_bytes_dropped
        ));
    }
    if capture.stderr_dropped != 0 {
        details.push_str(&format!(
            "Stderr history dropped {} bytes\n",
            capture.stderr_dropped
        ));
    }
    if let Some(error) = capture.error.as_deref() {
        details.push_str(&format!("Output incomplete: {error}\n"));
    }
    if !snapshot.diagnostics.is_empty() {
        details.push_str("Observation diagnostics:\n");
        for diagnostic in snapshot.diagnostics.iter().rev().take(3) {
            details.push_str(diagnostic);
            details.push('\n');
        }
    }
    details.push_str("Stderr tail:\n");
    details.push_str(&capture.stderr_tail);
    details
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

#[derive(Clone, Debug)]
enum SelectionAnchor {
    Node(String),
    Cluster(usize),
}

fn selection_anchor(layout: &GraphLayout, selected: usize) -> Option<SelectionAnchor> {
    if layout.is_collapsed_cluster(selected) {
        return layout.cluster_of(selected).map(SelectionAnchor::Cluster);
    }
    let source = *layout.sources(selected).first()?;
    Some(SelectionAnchor::Node(
        layout.all_nodes().get(source)?.id.clone(),
    ))
}

fn anchor_display(layout: &GraphLayout, anchor: &SelectionAnchor) -> Option<usize> {
    match anchor {
        SelectionAnchor::Node(id) => layout.display_index_for_node_id(id),
        SelectionAnchor::Cluster(cluster) => {
            layout.display_index_for_cluster(*cluster).or_else(|| {
                layout
                    .clusters()
                    .get(*cluster)?
                    .members
                    .first()
                    .and_then(|source| layout.display_index_for_source(*source))
            })
        }
    }
}

#[derive(Clone, Default)]
struct GraphViewport {
    offset: (u32, u32),
    selected: usize,
    pass_offset: usize,
    details_offset: usize,
}

pub struct ObservationView {
    description: WorkflowDescription,
    root: GraphLayout,
    // Retain only the currently inspected body, regardless of the number of scopes or views.
    bodies: BTreeMap<Vec<String>, GraphLayout>,
    viewports: BTreeMap<(Vec<String>, ConnectionView), GraphViewport>,
    collapsed: BTreeMap<Vec<String>, BTreeSet<usize>>,
    cluster_focus: BTreeMap<(Vec<String>, usize), usize>,
    state: ViewState,
}
impl ObservationView {
    pub fn new(description: &WorkflowDescription) -> Result<Self, RunError> {
        let root =
            GraphLayout::with_view(description, ConnectionView::Execution).context(GraphSnafu)?;
        Ok(Self {
            description: description.clone(),
            root,
            bodies: BTreeMap::new(),
            viewports: BTreeMap::new(),
            collapsed: BTreeMap::new(),
            cluster_focus: BTreeMap::new(),
            state: ViewState::default(),
        })
    }
    fn collapsed_at(&self, path: &[String]) -> BTreeSet<usize> {
        self.collapsed.get(path).cloned().unwrap_or_default()
    }
    fn rebuild_layouts(&mut self) -> Result<(), RunError> {
        let view = self.root.connection_view();
        let root_collapsed = self.collapsed_at(&[]);
        self.root = GraphLayout::with_view_and_collapsed(&self.description, view, &root_collapsed)
            .context(GraphSnafu)?;
        self.bodies.clear();
        if !self.state.path.is_empty() {
            let path = self.state.path.clone();
            let collapsed = self.collapsed_at(&path);
            let body = GraphLayout::from_body_with_view_and_collapsed(
                &self.description,
                &path,
                view,
                &collapsed,
            )
            .context(GraphSnafu)?;
            self.bodies.insert(path, body);
        }
        Ok(())
    }
    fn toggle_cluster(&mut self) -> Result<(), RunError> {
        let (cluster, collapsed, anchor, focus) = {
            let layout = self.state.layout(&self.root, &self.bodies);
            let selected = self.state.selected;
            let Some(cluster) = layout.cluster_of(selected) else {
                return Ok(());
            };
            let collapsed = layout.clusters()[cluster].collapsed;
            let focus = (!collapsed && !layout.is_collapsed_cluster(selected))
                .then(|| layout.sources(selected).first().copied())
                .flatten();
            let anchor =
                selection_anchor(layout, selected).unwrap_or(SelectionAnchor::Cluster(cluster));
            (cluster, collapsed, anchor, focus)
        };
        let path = self.state.path.clone();
        if let Some(focus) = focus {
            self.cluster_focus.insert((path.clone(), cluster), focus);
        }
        let set = self.collapsed.entry(path.clone()).or_default();
        if collapsed {
            set.remove(&cluster);
        } else {
            set.insert(cluster);
        }
        self.rebuild_layouts()?;
        let layout = self.state.layout(&self.root, &self.bodies);
        let selected = match &anchor {
            SelectionAnchor::Cluster(cluster) => self
                .cluster_focus
                .get(&(path, *cluster))
                .copied()
                .and_then(|source| layout.display_index_for_source(source))
                .or_else(|| anchor_display(layout, &anchor)),
            _ => anchor_display(layout, &anchor),
        };
        if let Some(selected) = selected {
            self.state.selected = selected;
        }
        self.state.reveal_selected = true;
        self.state.details_offset = 0;
        self.save_viewport();
        Ok(())
    }
    fn save_viewport(&mut self) {
        self.viewports.insert(
            (self.state.path.clone(), self.root.connection_view()),
            GraphViewport {
                offset: self.state.offset,
                selected: self.state.selected,
                pass_offset: self.state.pass_offset,
                details_offset: self.state.details_offset,
            },
        );
    }
    fn restore_viewport(&mut self) {
        let viewport = self
            .viewports
            .get(&(self.state.path.clone(), self.root.connection_view()))
            .cloned()
            .unwrap_or_default();
        self.state.offset = viewport.offset;
        let node_count = self.state.layout(&self.root, &self.bodies).nodes().len();
        self.state.selected = viewport.selected.min(node_count.saturating_sub(1));
        self.state.pass_offset = viewport.pass_offset;
        self.state.details_offset = viewport.details_offset;
        self.state.reveal_selected = true;
    }
    fn load_body(&mut self, path: &[String]) -> Result<(), RunError> {
        if !path.is_empty() && !self.bodies.contains_key(path) {
            let collapsed = self.collapsed_at(path);
            let layout = GraphLayout::from_body_with_view_and_collapsed(
                &self.description,
                path,
                self.root.connection_view(),
                &collapsed,
            )
            .context(GraphSnafu)?;
            self.bodies.insert(path.to_vec(), layout);
        }
        Ok(())
    }
    fn switch_connections(&mut self) -> Result<(), RunError> {
        let anchor = selection_anchor(
            self.state.layout(&self.root, &self.bodies),
            self.state.selected,
        );
        let view = self.root.connection_view().next();
        let root_collapsed = self.collapsed_at(&[]);
        let root = GraphLayout::with_view_and_collapsed(&self.description, view, &root_collapsed)
            .context(GraphSnafu)?;
        let body = if self.state.path.is_empty() {
            None
        } else {
            let collapsed = self.collapsed_at(&self.state.path);
            Some(
                GraphLayout::from_body_with_view_and_collapsed(
                    &self.description,
                    &self.state.path,
                    view,
                    &collapsed,
                )
                .context(GraphSnafu)?,
            )
        };
        self.save_viewport();
        self.root = root;
        self.bodies.clear();
        if let Some(body) = body {
            self.bodies.insert(self.state.path.clone(), body);
        }
        self.restore_viewport();
        if let Some(anchor) = anchor
            && let Some(selected) =
                anchor_display(self.state.layout(&self.root, &self.bodies), &anchor)
        {
            self.state.selected = selected;
        }
        Ok(())
    }
    fn prepare_navigation(&mut self, key: KeyCode) -> Result<(), RunError> {
        if key == KeyCode::Esc && self.state.details_visible {
            return Ok(());
        }
        let mut path = self.state.path.clone();
        match key {
            KeyCode::Char('l') => {
                if let Some(node) = self
                    .state
                    .layout(&self.root, &self.bodies)
                    .nodes()
                    .get(self.state.selected)
                {
                    path.push(node.id.clone());
                }
                if self.description.body(&path).is_none() {
                    return Ok(());
                }
            }
            KeyCode::Char('h') | KeyCode::Esc => {
                path.pop();
            }
            _ => return Ok(()),
        }
        self.load_body(&path)
    }
    fn handle_navigation(
        &mut self,
        key: KeyCode,
        snapshot: &StateSnapshot,
        traces: &TraceSnapshot,
        logs: &LogSnapshot,
    ) {
        if !self.state.traces.visible {
            if key == KeyCode::Char('c') {
                self.state.graph_notice = self
                    .toggle_cluster()
                    .err()
                    .map(|error| format!("Cluster fold unavailable: {error}"));
                return;
            }
            if key == KeyCode::Char('e') {
                self.state.graph_notice = self
                    .switch_connections()
                    .err()
                    .map(|error| format!("Connection view unavailable: {error}"));
                return;
            }
            if let Err(error) = self.prepare_navigation(key) {
                self.state.graph_notice = Some(format!("Body view unavailable: {error}"));
                return;
            }
            self.save_viewport();
        }
        let previous = self.state.path.clone();
        self.state
            .handle_key(key, snapshot, &self.root, &self.bodies, traces, logs);
        if self.state.path != previous {
            self.restore_viewport();
        }
        self.bodies.retain(|path, _| *path == self.state.path);
    }
    pub fn handle_key(&mut self, key: KeyCode, observation: &LocalObservation) {
        if self.state.handle_help(key) {
            return;
        }
        let reader = &observation.reader;
        let traces = reader.trace_snapshot();
        if !self
            .state
            .traces
            .handle_data_key(key, &traces, &reader.data_snapshot())
        {
            self.handle_navigation(
                key,
                &reader.snapshot_shared(),
                &traces,
                &reader.log_snapshot(),
            );
        }
    }
    pub fn dismiss_overlay(&mut self) -> bool {
        if self.state.help_visible {
            self.state.help_visible = false;
            true
        } else if !self.state.traces.visible && self.state.details_visible {
            self.state.details_visible = false;
            true
        } else {
            false
        }
    }
    pub fn render_execution(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        observation: &LocalObservation,
    ) {
        let snapshot = observation.reader.snapshot_shared();
        let elapsed = observation.elapsed();
        let phase = match snapshot.workflow_outcome {
            Some(mf_telemetry::event::Outcome::Succeeded) => "Succeeded".to_owned(),
            Some(mf_telemetry::event::Outcome::Failed) => "Failed".to_owned(),
            None => observation.status().map_or_else(
                || "Running".to_owned(),
                |status| format!("Exited {}", exit_code(status)),
            ),
        };
        let completed = snapshot
            .nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.status,
                    NodeStatus::Succeeded
                        | NodeStatus::Failed
                        | NodeStatus::Skipped
                        | NodeStatus::NotRun
                )
            })
            .count();
        // Follow root execution independently of the full inspector's navigation state.
        let mut state = ViewState::default();
        state.follow_latest(&snapshot, &self.root);
        let current = self
            .root
            .nodes()
            .get(state.selected)
            .map_or("", |node| node.id.as_str());
        let [header, graph, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(u16::from(area.height > 2)),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(format!(
                "Flow: {phase} | {completed}/{} | {current} | {:.1}s",
                snapshot.nodes.len(),
                elapsed.as_secs_f64(),
            )),
            header,
        );
        state.reveal(&self.root, graph);
        frame.render_widget(
            GraphView::new(&self.root)
                .reader(&observation.reader)
                .snapshot(&snapshot)
                .selected(state.selected)
                .offset(state.offset.0, state.offset.1)
                .elapsed_ns(elapsed.as_nanos().min(u64::MAX as u128) as u64),
            graph,
        );
        frame.render_widget(
            Paragraph::new(format!(
                "Observation: {:?}",
                snapshot.lifecycle.completeness
            ))
            .style(Style::default().fg(Color::DarkGray)),
            footer,
        );
    }
    pub fn draw(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Box<dyn Write>>>,
        observation: &LocalObservation,
    ) -> Result<(), RunError> {
        draw(
            terminal,
            &self.root,
            &self.bodies,
            &self.description,
            &observation.reader.snapshot_shared(),
            &observation.capture_view(),
            observation.elapsed(),
            &mut self.state,
            &observation.reader,
            observation.status(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for_outcome(run: &mut LocalRun) -> LocalOutcome {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(outcome) = run.poll().unwrap() {
                return outcome;
            }
            assert!(Instant::now() < deadline, "local capture did not finish");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn native_record_decoder_preserves_fragmented_utf8_and_validates_unique_keys() {
        let bytes = "{\"text\":\"é\"}\n\n{\"count\":2}".as_bytes();
        let mut decoder = NativeOutputDecoder::default();
        let mut records = Vec::new();
        for byte in bytes {
            decoder
                .feed(&[*byte], &mut |record| {
                    records.push(record);
                    Ok(())
                })
                .unwrap();
        }
        decoder
            .finish(&mut |record| {
                records.push(record);
                Ok(())
            })
            .unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["text"].as_str(), Some("é"));
        assert_eq!(records[1]["count"].as_u64(), Some(2));
        let error = NativeOutputDecoder::default()
            .feed(b"{\"x\":1,\"x\":2}\n", &mut |_| Ok(()))
            .unwrap_err();
        assert!(matches!(error, CaptureFailure::OutputRecord { .. }));
    }

    #[test]
    fn native_capture_delivers_records_before_process_completion() {
        let root = tempfile::tempdir().unwrap();
        let release = root.path().join("release");
        let mut command = Command::new("python3");
        command.arg("-c").arg("import os,sys,time;print('{\"value\":1}',flush=True)\nwhile not os.path.exists(sys.argv[1]): time.sleep(0.005)\nprint('{\"value\":2}',flush=True)").arg(&release);
        let mut run =
            LocalRun::start_stream(command, scope_fixture(), RunId::new(), Default::default())
                .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let first = loop {
            if let Ok(record) = run.try_record() {
                break record;
            }
            assert!(run.poll().unwrap().is_none());
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(first["value"].as_u64(), Some(1));
        assert!(run.child.child.try_wait().unwrap().is_none());
        std::fs::write(&release, "release").unwrap();
        let outcome = wait_for_outcome(&mut run);
        assert!(outcome.status.success());
        assert!(outcome.capture_failure.is_none());
        assert_eq!(run.try_record().unwrap()["value"].as_u64(), Some(2));
        assert!(outcome.stdout.is_empty());
    }

    #[test]
    fn malformed_native_output_keeps_preceding_records_and_typed_diagnostics() {
        let mut command = Command::new("python3");
        command
            .arg("-c")
            .arg("print('{\"value\":1}');print('invalid-json')");
        let mut run =
            LocalRun::start_stream(command, scope_fixture(), RunId::new(), Default::default())
                .unwrap();
        let outcome = wait_for_outcome(&mut run);
        assert!(matches!(
            outcome.capture_failure.as_deref(),
            Some(CaptureFailure::OutputRecord { .. })
        ));
        assert_eq!(run.try_record().unwrap()["value"].as_u64(), Some(1));
    }

    #[test]
    fn a_transient_signal_failure_keeps_reaped_output() {
        let mut command = Command::new("python3");
        command.arg("-c").arg("print('{\"text\":\"complete\"}')");
        let mut run = LocalRun::start(command, scope_fixture(), RunId::new(), 1024).unwrap();
        while run.child.child.try_wait().unwrap().is_none() {
            thread::sleep(Duration::from_millis(1));
        }
        run.kill_error = Some((
            io::Error::from_raw_os_error(nix::libc::EPERM),
            Instant::now() - DRAIN_TIMEOUT,
        ));
        let outcome = wait_for_outcome(&mut run);
        assert!(outcome.status.success());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&outcome.stdout).unwrap()["text"],
            "complete"
        );
    }

    #[test]
    fn a_persistent_signal_failure_remains_a_typed_supervision_error() {
        let mut command = Command::new("sleep");
        command.arg("20");
        let mut run = LocalRun::start(command, scope_fixture(), RunId::new(), 1024).unwrap();
        run.killed = true;
        run.kill_error = Some((
            io::Error::from_raw_os_error(nix::libc::EPERM),
            Instant::now() - DRAIN_TIMEOUT,
        ));
        assert!(
            matches!(run.poll(), Err(RunError::Supervise { source, .. }) if source.kind() == io::ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn cancellation_interrupts_native_capture_under_receiver_pressure() {
        for parent in [false, true] {
            let mut command = Command::new("python3");
            command.arg("-c").arg("import json,time;[print(json.dumps({'value':i}),flush=True) for i in range(1000)];time.sleep(20)");
            let cancellation = mf_runtime::StreamCancellation::default();
            let mut run = LocalRun::start_stream(
                command,
                scope_fixture(),
                RunId::new(),
                cancellation.clone(),
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !run.capture.processing() {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(5));
            }
            if parent {
                cancellation.cancel(mf_runtime::StreamError::Execution {
                    message: "host cancelled".into(),
                });
            } else {
                run.cancel();
            }
            let outcome = wait_for_outcome(&mut run);
            assert!(outcome.interrupted);
            assert!(outcome.stdout.is_empty());
            assert_eq!(cancellation.failure().is_some(), parent);
        }
    }
    use mf_runtime::StdinRequirement;
    use mf_telemetry::{
        Count,
        description::{
            ClusterDescription, DataEdge, ExecutionDescription, ExecutionMode, LoopBodyDescription,
            NodeDescription, WorkflowDescription, WorkflowDescriptionVersion,
        },
        event::{Event as LifecycleKind, LifecycleEvent, LoopPathEntry},
        identity::WorkflowId,
    };

    fn scope_fixture() -> WorkflowDescription {
        serde_json::from_value(serde_json::json!({
            "version":"2026-09-29","workflow_id":format!("sha256:{}","e".repeat(64)),
            "nodes":[{"id":"first","kind":"workflow.loop"},{"id":"second","kind":"workflow.loop"}],
            "data_edges":[],"control_edges":[],"execution_order":["first","second"],
            "loop_bodies":(["first","second"].map(|id| serde_json::json!({
                "path":[id],"nodes":[{"id":"%loop","kind":"%loop"},{"id":"source","kind":"fixture.task"},{"id":"target","kind":"fixture.task"}],
                "data_edges":[{"from_node":"source","from_output":"value","to_node":"target","to_input":"input"}],
                "control_edges":[],"execution_order":["%loop","source","target"]
            })))
        })).unwrap()
    }

    #[test]
    fn cluster_folding_rebuilds_the_layout_and_preserves_the_selected_member() {
        let description = WorkflowDescription {
            version: WorkflowDescriptionVersion::V2026_10_23,
            workflow_id: WorkflowId::try_from(format!("sha256:{}", "f".repeat(64))).unwrap(),
            execution: Some(ExecutionDescription {
                mode: ExecutionMode::Single,
            }),
            nodes: ["read", "parse", "publish"]
                .map(|id| NodeDescription {
                    id: id.into(),
                    kind: "fixture.task".into(),
                })
                .to_vec(),
            data_edges: vec![
                DataEdge {
                    exclusive: false,
                    from_node: "read".into(),
                    from_output: "value".into(),
                    to_node: "parse".into(),
                    to_input: "input".into(),
                },
                DataEdge {
                    exclusive: false,
                    from_node: "parse".into(),
                    from_output: "value".into(),
                    to_node: "publish".into(),
                    to_input: "input".into(),
                },
            ],
            control_edges: Vec::new(),
            order_edges: Vec::new(),
            execution_order: vec!["read".into(), "parse".into(), "publish".into()],
            loop_bodies: Vec::new(),
            clusters: vec![ClusterDescription {
                label: "Load".into(),
                annotation: "Read and parse the input.".into(),
                nodes: vec!["read".into(), "parse".into()],
            }],
        };
        let mut view = ObservationView::new(&description).unwrap();
        view.state.selected = view.root.display_index_for_node_id("parse").unwrap();
        view.toggle_cluster().unwrap();
        assert!(view.root.clusters()[0].collapsed);
        assert!(view.root.is_collapsed_cluster(view.state.selected));
        assert_eq!(view.root.sources(view.state.selected), &[0, 1]);

        view.toggle_cluster().unwrap();
        assert!(!view.root.clusters()[0].collapsed);
        assert_eq!(view.root.nodes()[view.state.selected].id, "parse");
        assert_eq!(view.root.nodes().len(), description.nodes.len());
    }

    #[test]
    fn chat_title_usage_folding_and_view_changes_preserve_selection() {
        let definition = mf_compiler::WorkflowDefinition::from_json(include_str!(
            "../../../examples/chat-title-flow.json"
        ))
        .unwrap();
        let plan = mf_compiler::plan_definition(&definition).unwrap();
        let description = mf_compiler::describe_compiled(&plan).unwrap();
        let snapshot = crate::state::SessionState::new(description.clone(), RunId::new())
            .unwrap()
            .snapshot();
        let mut view = ObservationView::new(&description).unwrap();
        let traces = TraceSnapshot::default();
        let logs = LogSnapshot::default();
        view.state.selected = view.root.display_index_for_node_id("usages").unwrap();
        view.handle_navigation(KeyCode::Char('c'), &snapshot, &traces, &logs);
        assert!(view.state.graph_notice.is_none());
        assert!(view.root.is_collapsed_cluster(view.state.selected));
        for _ in 0..3 {
            view.handle_navigation(KeyCode::Char('e'), &snapshot, &traces, &logs);
            assert!(view.state.graph_notice.is_none());
            assert!(view.root.is_collapsed_cluster(view.state.selected));
        }
        view.handle_navigation(KeyCode::Char('c'), &snapshot, &traces, &logs);
        assert!(view.state.graph_notice.is_none());
        assert_eq!(view.root.nodes()[view.state.selected].id, "usages");
    }

    #[test]
    fn scrolling_reveals_nodes_with_minimal_movement_and_handles_small_viewports() {
        let node = GraphRect {
            x: 40,
            y: 20,
            width: 24,
            height: 4,
        };
        assert_eq!(scroll_into_view((0, 0), node, (50, 22)), (14, 2));
        assert_eq!(scroll_into_view((14, 2), node, (50, 22)), (14, 2));
        assert_eq!(scroll_into_view((60, 30), node, (50, 22)), (40, 20));
        assert_eq!(scroll_into_view((0, 0), node, (10, 2)), (40, 20));
        assert_eq!(scroll_into_view((14, 2), node, (0, 0)), (14, 2));
    }

    #[test]
    fn navigation_reveals_selection_but_redraws_preserve_manual_panning() {
        let description = scope_fixture();
        let snapshot = crate::state::SessionState::new(description.clone(), RunId::new())
            .unwrap()
            .snapshot();
        let mut view = ObservationView::new(&description).unwrap();
        let traces = TraceSnapshot::default();
        let logs = LogSnapshot::default();
        let area = Rect::new(5, 3, 26, 6);
        view.state.reveal(&view.root, area);
        for key in [
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Char('k'),
            KeyCode::Char('j'),
        ] {
            view.state.offset = (1000, 1000);
            view.handle_navigation(key, &snapshot, &traces, &logs);
            view.state.reveal(&view.root, area);
            let rect = view.root.nodes()[view.state.selected].rect;
            assert!(view.state.offset.0 <= rect.x);
            assert!(view.state.offset.1 <= rect.y);
            assert!(view.state.offset.0 + u32::from(area.width) >= rect.x + rect.width);
            assert!(view.state.offset.1 + u32::from(area.height) >= rect.y + rect.height);
        }
        view.handle_navigation(KeyCode::Right, &snapshot, &traces, &logs);
        let panned = view.state.offset;
        view.state.reveal(&view.root, area);
        assert_eq!(view.state.offset, panned);
        view.state.reveal(&view.root, Rect::new(0, 0, 10, 2));
        let rect = view.root.nodes()[view.state.selected].rect;
        assert_eq!(view.state.offset, (rect.x, rect.y));
    }

    #[test]
    fn new_events_follow_visible_nodes_without_overriding_manual_selection_on_redraw() {
        let description = scope_fixture();
        let mut snapshot = crate::state::SessionState::new(description.clone(), RunId::new())
            .unwrap()
            .snapshot();
        let mut view = ObservationView::new(&description).unwrap();
        snapshot.latest_node_event = Some(crate::state::NodeEventFocus {
            sequence: Count::try_from(2).unwrap(),
            node: mf_telemetry::event::NodeIdentity {
                id: "target".into(),
                kind: "fixture.task".into(),
                path: vec![LoopPathEntry {
                    loop_id: "second".into(),
                    index: Count::ZERO,
                }],
            },
        });
        view.state.follow_latest(&snapshot, &view.root);
        assert_eq!(view.root.nodes()[view.state.selected].id, "second");
        assert!(view.state.reveal_selected);
        view.state.selected = 0;
        view.state.reveal_selected = false;
        view.state.follow_latest(&snapshot, &view.root);
        assert_eq!(view.state.selected, 0);
        assert!(!view.state.reveal_selected);

        view.state.path = vec!["second".into()];
        view.load_body(&["second".into()]).unwrap();
        let layout = view.state.layout(&view.root, &view.bodies);
        snapshot.latest_node_event.as_mut().unwrap().sequence = Count::try_from(3).unwrap();
        view.state.follow_latest(&snapshot, layout);
        assert_eq!(layout.nodes()[view.state.selected].id, "target");
        view.state.pass_offset = 1;
        view.state.selected = 0;
        snapshot.latest_node_event.as_mut().unwrap().sequence = Count::try_from(4).unwrap();
        view.state.follow_latest(&snapshot, layout);
        assert_eq!(view.state.selected, 0);
        view.state.pass_offset = 0;
        snapshot.latest_node_event.as_mut().unwrap().sequence = Count::try_from(5).unwrap();
        snapshot.latest_node_event.as_mut().unwrap().node.path[0].loop_id = "first".into();
        view.state.follow_latest(&snapshot, layout);
        assert_eq!(view.state.selected, 0);
    }

    #[test]
    fn incomplete_invocations_remain_visible_in_node_and_workflow_details() {
        use crate::state::{NodeStatus, StreamInvocationObservation, StreamSnapshot};
        use mf_telemetry::stream::{StreamIdentity, StreamTrigger};

        let mut snapshot = crate::state::SessionState::new(scope_fixture(), RunId::new())
            .unwrap()
            .snapshot();
        let node = &mut snapshot.nodes[0];
        node.status = NodeStatus::Interrupted;
        node.possibly_missing_events = true;
        node.last_known = Some(NodeStatus::Running);
        snapshot.stream = Some(StreamSnapshot {
            invocations: vec![StreamInvocationObservation {
                identity: StreamIdentity {
                    invocation: 1,
                    trigger: StreamTrigger::Startup,
                    message: None,
                    parent: None,
                },
                path: vec![],
                node: node.clone(),
                loop_summary: None,
                completed_passes: 0,
                active_pass: None,
            }],
            ..Default::default()
        });
        let capture = CaptureView {
            stdout_open: false,
            stderr_open: false,
            stderr_tail: String::new(),
            stderr_dropped: 0,
            error: None,
        };
        let text = details_text(snapshot.nodes.first(), None, &snapshot, &capture, "");
        assert!(text.contains("Status: Interrupted"));
        assert!(text.contains("Node observation may be incomplete"));
        assert!(text.contains("Unresolved invocation outcomes: 1"));

        let text = details_text(None, None, &snapshot, &capture, "");
        assert!(!text.contains("Node observation may be incomplete"));
        assert!(text.contains("Unresolved invocation outcomes: 1"));
    }

    #[test]
    fn observation_elapsed_stops_after_process_completion() {
        let mut command = Command::new("sh");
        command.args(["-c", "exit 0"]);
        let mut run = LocalRun::start(command, scope_fixture(), RunId::new(), 1024).unwrap();
        let observation = run.observation();
        let elapsed = observation.elapsed();
        thread::sleep(Duration::from_millis(10));
        assert!(observation.elapsed() > elapsed);

        let outcome = (0..500).find_map(|_| {
            thread::sleep(Duration::from_millis(10));
            run.poll().unwrap()
        });
        assert!(outcome.is_some(), "process did not finish");
        let elapsed = observation.elapsed();
        thread::sleep(Duration::from_millis(20));
        assert_eq!(observation.elapsed(), elapsed);
    }

    #[test]
    fn connection_navigation_restores_viewports_and_bounds_retained_layouts() {
        let description = scope_fixture();
        let snapshot = crate::state::SessionState::new(description.clone(), RunId::new())
            .unwrap()
            .snapshot();
        let mut view = ObservationView::new(&description).unwrap();
        let traces = TraceSnapshot::default();
        let logs = LogSnapshot::default();
        assert_eq!(view.root.connection_view(), ConnectionView::Execution);
        assert!(view.bodies.is_empty());
        let geometry = view.root.nodes().to_vec();
        view.state.selected = 1;
        view.state.offset = (12, 8);
        view.handle_navigation(KeyCode::Char('e'), &snapshot, &traces, &logs);
        assert_eq!(view.state.selected, 1);
        view.state.offset = (30, 16);
        view.handle_navigation(KeyCode::Char('e'), &snapshot, &traces, &logs);
        view.handle_navigation(KeyCode::Char('e'), &snapshot, &traces, &logs);
        assert_eq!(view.state.offset, (12, 8));
        assert_eq!(view.state.selected, 1);
        assert_eq!(view.root.nodes(), geometry);
        for _ in 0..4 {
            for selected in [0, 1] {
                view.state.selected = selected;
                view.handle_navigation(KeyCode::Char('l'), &snapshot, &traces, &logs);
                assert_eq!(view.bodies.len(), 1);
                assert_eq!(view.state.path.len(), 1);
                view.state.selected = 2;
                view.state.offset = (7, 9);
                for _ in 0..3 {
                    view.handle_navigation(KeyCode::Char('e'), &snapshot, &traces, &logs);
                    assert_eq!(view.bodies.len(), 1);
                    assert_eq!(view.state.selected, 2);
                }
                assert_eq!(view.state.offset, (7, 9));
                view.handle_navigation(KeyCode::Char('h'), &snapshot, &traces, &logs);
                assert!(view.state.path.is_empty());
                assert!(view.bodies.is_empty());
                assert_eq!(view.state.selected, selected);
            }
        }
        assert!(view.viewports.len() <= 9);
        view.state.traces.visible = true;
        let mode = view.root.connection_view();
        view.handle_navigation(KeyCode::Char('e'), &snapshot, &traces, &logs);
        assert_eq!(view.root.connection_view(), mode);
    }

    #[test]
    fn transitive_summary_and_details_preserve_hidden_loop_body_bindings() {
        let mut description = scope_fixture();
        let body = &mut description.loop_bodies[0];
        for target in ["source", "target"] {
            body.data_edges.push(mf_telemetry::description::DataEdge {
                exclusive: false,
                from_node: "%loop".into(),
                from_output: "value".into(),
                to_node: target.into(),
                to_input: "direct".into(),
            });
        }
        body.control_edges
            .push(mf_telemetry::description::ControlEdge {
                from_node: "source".into(),
                from_output: "ready".into(),
                to_node: "target".into(),
            });
        let path = vec!["first".into()];
        let layout =
            GraphLayout::from_body_with_view(&description, &path, ConnectionView::Execution)
                .unwrap();
        assert_eq!(
            connection_summary(&layout),
            "Execution: 1 data mappings hidden | 1 transitive dependencies hidden"
        );
        let node = NodeObservation::pending("target", "fixture.task");
        let details = binding_details(&description, &path, Some(&node));
        assert!(details.contains("%loop.value"));
        assert!(details.contains("source.value"));
        assert!(details.contains("source.ready"));
    }

    #[test]
    fn binding_details_explain_hidden_references_and_do_not_guess_selection() {
        let mut description = scope_fixture();
        description.nodes = vec![
            NodeDescription {
                id: "source".into(),
                kind: "fixture.task".into(),
            },
            NodeDescription {
                id: "target".into(),
                kind: "fixture.task".into(),
            },
        ];
        description.execution_order = vec!["source".into(), "target".into()];
        description.loop_bodies.clear();
        description.data_edges = ["first", "second"]
            .map(|port| mf_telemetry::description::DataEdge {
                exclusive: true,
                from_node: "source".into(),
                from_output: port.into(),
                to_node: "target".into(),
                to_input: "input".into(),
            })
            .to_vec();
        let layout = GraphLayout::with_view(&description, ConnectionView::Execution).unwrap();
        assert_eq!(connection_summary(&layout), "Connections: Execution");
        assert_eq!(layout.edges().len(), 1);
        assert_eq!(layout.edges()[0].members.len(), 2);
        let mut node = NodeObservation::pending("target", "fixture.task");
        let text = binding_details(&description, &[], Some(&node));
        assert!(text.contains("input [exclusive]"));
        assert!(text.contains("source.first"));
        assert!(text.contains("source.second"));
        assert!(text.contains("selected: unknown"));
        node.input_selections
            .push(mf_telemetry::event::InputSelection {
                input: "input".into(),
                source_node: "source".into(),
                source_output: "second".into(),
            });
        assert!(
            binding_details(&description, &[], Some(&node)).contains("selected: source.second")
        );
        node.possibly_missing_events = true;
        assert!(binding_details(&description, &[], Some(&node)).contains("selected: unknown"));
        node.possibly_missing_events = false;
        node.input_selections.clear();
        node.skip_causes = ["first", "second"]
            .map(|port| mf_telemetry::event::SkipCause::Output {
                source_node: "source".into(),
                source_output: port.into(),
            })
            .to_vec();
        assert!(
            binding_details(&description, &[], Some(&node))
                .contains("none (all candidates skipped)")
        );
        let source = NodeObservation::pending("source", "fixture.task");
        assert!(
            binding_details(&description, &[], Some(&source)).contains("target.input [exclusive]")
        );
    }

    #[test]
    fn iteration_bodies_are_static_and_cannot_be_used_as_loop_lifecycle_paths() {
        let mut description = scope_fixture();
        description.version = WorkflowDescriptionVersion::V2026_10_19;
        description.execution = Some(mf_telemetry::description::ExecutionDescription {
            mode: mf_telemetry::description::ExecutionMode::Single,
        });
        description.nodes[0].kind = "builtin.iteration".into();
        description.loop_bodies[0].nodes[0].id = mf_runtime::ITERATION_INPUT_ID.into();
        description.loop_bodies[0].nodes[0].kind = mf_runtime::ITERATION_INPUT_KIND.into();
        description.loop_bodies[0].execution_order[0] = mf_runtime::ITERATION_INPUT_ID.into();
        description.validate().unwrap();
        let path = vec!["first".into()];
        assert!(description.body(&path).is_some());
        assert!(description.loop_body(&path).is_none());
        let snapshot = crate::state::SessionState::new(description.clone(), RunId::new())
            .unwrap()
            .snapshot();
        let mut view = ObservationView::new(&description).unwrap();
        view.handle_navigation(
            KeyCode::Char('l'),
            &snapshot,
            &TraceSnapshot::default(),
            &LogSnapshot::default(),
        );
        assert_eq!(view.state.path, path);
        let nodes = body_observations(view.state.layout(&view.root, &view.bodies), None, true);
        assert!(
            nodes
                .iter()
                .all(|node| node.status == crate::state::NodeStatus::Unknown)
        );
        let event = LifecycleEvent {
            workflow_id: description.workflow_id.clone(),
            run_id: RunId::new(),
            sequence: Count::try_from(2).unwrap(),
            event: LifecycleKind::LoopPassStarted {
                path: vec![LoopPathEntry {
                    loop_id: "first".into(),
                    index: Count::ZERO,
                }],
                elapsed_ns: Count::ZERO,
            },
        };
        assert!(event.validate_for(&description).is_err());
    }

    fn source_runner(directory: &Path, resource: Option<StdinRequirement>) -> PathBuf {
        use std::{fs, os::unix::fs::PermissionsExt};
        let runner = directory.join("runner");
        let definition = serde_json::json!({"version":"2026-10-03", "dependencies":{},
            "execution":{},
            "nodes":[{"id":"source/id", "kind":"third-party.dynamic_source"}]});
        let mut stdin = serde_json::Map::new();
        if let Some(resource) = resource {
            stdin.insert("source/id".into(), serde_json::json!(resource));
        }
        let manifest = mf_runtime::WorkflowManifest {
            version: mf_runtime::WorkflowManifestVersion::V2026_10_16,
            definition: serde_json::from_value(definition).unwrap(),
            execution_order: vec!["source/id".into()],
            loop_orders: vec![],
            input_schema: serde_json::from_value(serde_json::json!({
                "inputs":{"source/id":{"path":{"type":"string", "required":true}}}, "stdin":stdin
            }))
            .unwrap(),
        };
        fs::write(
            &runner,
            crate::test_support::elf(Some(&manifest.to_bytes().unwrap()), 1),
        )
        .unwrap();
        fs::set_permissions(&runner, fs::Permissions::from_mode(0o600)).unwrap();
        runner
    }

    fn valid_options() -> RunOptions {
        RunOptions {
            inputs: Some(r#"{"source/id.path":"data.txt"}"#.into()),
            ..Default::default()
        }
    }

    #[test]
    fn details_window_wraps_cell_width_and_reaches_rows_beyond_u16() {
        let text = (0..70_000)
            .map(|index| format!("connection-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (window, offset) = details_window(&text, 40, 4, 69_999);
        assert_eq!(offset, 69_996);
        assert!(window.contains("connection-69999"));
        let (window, offset) = details_window("wide-🦀-end", 6, 3, 0);
        assert_eq!(offset, 0);
        for line in window.lines() {
            assert!(ratatui::text::Span::raw(line).width() <= 6);
        }
        assert!(window.contains('🦀'));
        assert_eq!(details_window("text", 0, 4, 10), (String::new(), 0));
    }

    #[test]
    fn short_details_pages_preserve_the_complete_cluster_annotation() {
        let description = scope_fixture();
        let layout = GraphLayout::new(&description).unwrap();
        let session = crate::state::SessionState::new(description, RunId::new()).unwrap();
        let cluster = GraphCluster {
            label: "Read and validate the complete configuration".into(),
            annotation: "Read every configuration field.\n\tPreserve a\u{301} and 🦀 across narrow pages.\nFinish with the complete annotation.".into(),
            members: vec![0],
            rect: GraphRect { x: 0, y: 0, width: 24, height: 4 },
            collapsed: true,
        };
        let text = collapsed_cluster_details(&cluster, &[0], &session.snapshot().nodes);
        assert!(text.contains("    Preserve"));
        let mut state = ViewState {
            details_visible: true,
            details_height: 2,
            ..Default::default()
        };
        let mut rows = BTreeMap::new();
        loop {
            let (page, offset) =
                details_window(&text, 12, state.details_height, state.details_offset);
            for (row, line) in page.split('\n').enumerate() {
                rows.insert(offset + row, line.to_owned());
            }
            if offset < state.details_offset {
                break;
            }
            state.handle_key(
                KeyCode::PageDown,
                &session.snapshot(),
                &layout,
                &BTreeMap::new(),
                &TraceSnapshot::default(),
                &LogSnapshot::default(),
            );
        }
        assert_eq!(
            rows.into_values().collect::<String>(),
            text.replace('\n', "")
        );
        let previous_offset = state.details_offset;
        state.handle_key(
            KeyCode::PageUp,
            &session.snapshot(),
            &layout,
            &BTreeMap::new(),
            &TraceSnapshot::default(),
            &LogSnapshot::default(),
        );
        assert_eq!(state.details_offset, previous_offset - state.details_height);
    }

    #[test]
    fn shortcut_help_consumes_navigation_until_dismissed() {
        let mut state = ViewState::default();
        assert!(!state.help_visible);
        assert!(state.handle_help(KeyCode::Char('?')));
        assert!(state.help_visible);
        for key in [KeyCode::Char('v'), KeyCode::Char('d'), KeyCode::Tab] {
            assert!(state.handle_help(key));
            assert!(state.help_visible);
        }
        assert!(!state.traces.visible);
        assert!(!state.details_visible);
        for close in [KeyCode::Esc, KeyCode::Enter, KeyCode::Char('q')] {
            assert!(state.handle_help(close));
            assert!(!state.help_visible);
            assert!(!state.handle_help(KeyCode::Tab));
            assert!(state.handle_help(KeyCode::Char('?')));
        }
        assert!(state.handle_help(KeyCode::Char('?')));
        assert!(!state.help_visible);
    }

    #[test]
    fn routing_warnings_are_independent_of_observation_integrity() {
        let mut description = crate::test_support::sample_manifest()
            .description()
            .unwrap();
        description.nodes = ["source", "target"]
            .into_iter()
            .map(|id| NodeDescription {
                id: id.into(),
                kind: "fixture.node".into(),
            })
            .collect();
        description.execution_order = vec!["source".into(), "target".into()];
        description
            .data_edges
            .push(mf_telemetry::description::DataEdge {
                exclusive: false,
                from_node: "source".into(),
                from_output: "value".into(),
                to_node: "target".into(),
                to_input: "input".into(),
            });
        let session = crate::state::SessionState::new(description.clone(), RunId::new()).unwrap();
        let before = session.snapshot().lifecycle.completeness;
        let layout = GraphLayout::with_routing_limits(
            &description,
            crate::graph::routing::RoutingLimits {
                max_conflict_checks: 0,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(routing_warning(&layout).contains("Connections: 1 unresolved"));
        assert!(
            layout
                .routing_details(0)
                .contains("source.value -> target.input")
        );
        assert_eq!(session.snapshot().lifecycle.completeness, before);
        let mut view = ViewState::default();
        assert!(!view.details_visible);
        view.handle_key(
            KeyCode::PageDown,
            &session.snapshot(),
            &layout,
            &BTreeMap::new(),
            &TraceSnapshot::default(),
            &LogSnapshot::default(),
        );
        assert_eq!(view.details_offset, 0);
        view.handle_key(
            KeyCode::Char('d'),
            &session.snapshot(),
            &layout,
            &BTreeMap::new(),
            &TraceSnapshot::default(),
            &LogSnapshot::default(),
        );
        assert!(view.details_visible);
        view.details_height = 10;
        view.handle_key(
            KeyCode::PageDown,
            &session.snapshot(),
            &layout,
            &BTreeMap::new(),
            &TraceSnapshot::default(),
            &LogSnapshot::default(),
        );
        assert_eq!(view.details_offset, 10);
        view.handle_key(
            KeyCode::Tab,
            &session.snapshot(),
            &layout,
            &BTreeMap::new(),
            &TraceSnapshot::default(),
            &LogSnapshot::default(),
        );
        assert_eq!(view.details_offset, 0);
    }

    #[test]
    fn preflight_validates_interfaces_and_preserves_owned_parameters() {
        use std::{fs, os::unix::fs::PermissionsExt};
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("arguments.json");
        fs::write(&path, valid_options().inputs.unwrap()).unwrap();
        let runner = source_runner(root.path(), None);
        let prepared = prepare_launch(
            &runner,
            &RunOptions {
                inputs_file: Some(path.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        fs::remove_file(&path).unwrap();
        let private = prepared.arguments.as_ref().unwrap();
        assert_eq!(
            private.as_file().metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        let temporary_path = private.path().to_owned();
        assert_eq!(
            WorkflowArguments::from_file(private.path()).unwrap(),
            WorkflowArguments::from_json(valid_options().inputs.unwrap().as_bytes()).unwrap()
        );
        drop(prepared);
        assert!(!temporary_path.exists());
        let runner = source_runner(root.path(), None);
        for input in [
            "{}",
            r#"{"source/id.path":42}"#,
            r#"{"source/id.path":"a","source/id.path":"b"}"#,
            r#"{"source/id.path":"a","source/id.extra":null}"#,
        ] {
            assert!(
                prepare_launch(
                    &runner,
                    &RunOptions {
                        inputs: Some(input.into()),
                        ..Default::default()
                    }
                )
                .is_err()
            );
        }
        let mut options = valid_options();
        options.inputs_file = Some(path);
        assert!(prepare_launch(&runner, &options).is_err());
        options.inputs_file = None;
        options.arguments = vec!["--source/id.path=data.txt".into()];
        assert!(prepare_launch(&runner, &options).is_err());
        options.arguments.clear();
        options.inputs = Some(" ".repeat(MAX_WORKFLOW_INPUT_BYTES + 1));
        assert!(matches!(
            prepare_launch(&runner, &options),
            Err(RunError::Inputs {
                source: WorkflowInputError::TooLarge { .. }
            })
        ));
    }

    #[test]
    fn preflight_rejects_workflow_stdin() {
        let root = tempfile::tempdir().unwrap();
        let runner = source_runner(root.path(), Some(StdinRequirement::Always));
        assert!(
            prepare_launch(&runner, &valid_options())
                .err()
                .unwrap()
                .to_string()
                .contains("cannot supply workflow stdin")
        );
    }

    #[test]
    fn preflight_resolves_conditional_stdin_without_starting_a_runner() {
        let root = tempfile::tempdir().unwrap();
        let runner = source_runner(
            root.path(),
            Some(StdinRequirement::UnlessInput("path".into())),
        );
        let mut manifest = read_manifest(&runner).unwrap();
        manifest
            .input_schema
            .inputs
            .get_mut("source/id")
            .unwrap()
            .get_mut("path")
            .unwrap()
            .required = false;
        std::fs::write(
            &runner,
            crate::test_support::elf(Some(&manifest.to_bytes().unwrap()), 1),
        )
        .unwrap();
        assert!(prepare_launch(&runner, &valid_options()).is_ok());
        assert!(
            prepare_launch(&runner, &RunOptions::default())
                .err()
                .unwrap()
                .to_string()
                .contains("cannot supply workflow stdin")
        );
        let mut corrupt = std::fs::read(&runner).unwrap();
        *corrupt.last_mut().unwrap() = 0xff;
        std::fs::write(&runner, corrupt).unwrap();
        assert!(matches!(
            prepare_launch(&runner, &valid_options()),
            Err(RunError::Manifest { .. })
        ));
    }

    #[test]
    fn preflight_rejects_missing_manifest_without_starting_a_runner() {
        let root = tempfile::tempdir().unwrap();
        let runner = root.path().join("legacy");
        for bytes in [
            crate::test_support::elf(None, 0),
            crate::test_support::macho(None, 0),
        ] {
            std::fs::write(&runner, bytes).unwrap();
            assert!(matches!(
                prepare_launch(&runner, &RunOptions::default()),
                Err(RunError::Manifest {
                    source: ManifestReadError::MissingManifest
                })
            ));
        }
    }

    #[test]
    fn loop_views_preserve_navigation_and_compacted_stream_totals() {
        let description = WorkflowDescription {
            order_edges: Vec::new(),
            version: WorkflowDescriptionVersion::V2026_09_29,
            workflow_id: WorkflowId::try_from(format!("sha256:{}", "c".repeat(64))).unwrap(),
            nodes: vec![NodeDescription {
                id: "repeat".into(),
                kind: "workflow.loop".into(),
            }],
            data_edges: vec![],
            control_edges: vec![],
            execution_order: vec!["repeat".into()],
            execution: None,
            loop_bodies: vec![LoopBodyDescription {
                order_edges: Vec::new(),
                path: vec!["repeat".into()],
                nodes: vec![
                    NodeDescription {
                        id: "%loop".into(),
                        kind: "%loop".into(),
                    },
                    NodeDescription {
                        id: "child".into(),
                        kind: "fixture.node".into(),
                    },
                ],
                data_edges: vec![mf_telemetry::description::DataEdge {
                    exclusive: false,
                    from_node: "%loop".into(),
                    from_output: "value".into(),
                    to_node: "child".into(),
                    to_input: "input".into(),
                }],
                control_edges: vec![],
                execution_order: vec!["%loop".into(), "child".into()],
                clusters: Vec::new(),
            }],
            clusters: Vec::new(),
        };
        let root = GraphLayout::new(&description).unwrap();
        let bodies = BTreeMap::from([(
            vec!["repeat".into()],
            GraphLayout::from_loop_body(&description, &["repeat".into()]).unwrap(),
        )]);
        let before_nodes = bodies[&vec!["repeat".to_owned()]].nodes().to_vec();
        let before_routes = bodies[&vec!["repeat".to_owned()]].routes().to_vec();
        let before_crossings = bodies[&vec!["repeat".to_owned()]].crossings().to_vec();
        let run_id = RunId::new();
        let mut session = crate::state::SessionState::new(description.clone(), run_id).unwrap();
        let count = |value| Count::try_from(value).unwrap();
        for (sequence, event) in [
            (
                1,
                LifecycleKind::WorkflowStarted {
                    node_count: count(3),
                    elapsed_ns: Count::ZERO,
                },
            ),
            (
                2,
                LifecycleKind::LoopPassStarted {
                    path: vec![LoopPathEntry {
                        loop_id: "repeat".into(),
                        index: Count::ZERO,
                    }],
                    elapsed_ns: count(1),
                },
            ),
            (
                3,
                LifecycleKind::LoopPassStarted {
                    path: vec![LoopPathEntry {
                        loop_id: "repeat".into(),
                        index: count(1),
                    }],
                    elapsed_ns: count(2),
                },
            ),
        ] {
            session
                .apply(LifecycleEvent {
                    workflow_id: description.workflow_id.clone(),
                    run_id,
                    sequence: count(sequence),
                    event,
                })
                .unwrap();
        }
        let mut snapshot = session.snapshot();
        for (index, pass) in snapshot.loop_passes.iter_mut().enumerate() {
            pass.stream_invocation = Some(index as u64 + 10);
        }
        let mut view = ViewState::default();
        view.handle_key(
            KeyCode::Char('l'),
            &snapshot,
            &root,
            &bodies,
            &TraceSnapshot::default(),
            &LogSnapshot::default(),
        );
        assert_eq!(view.path, ["repeat"]);
        assert_eq!(view.layout(&root, &bodies).nodes()[0].id, "%loop");
        assert_eq!(view.pass(&snapshot).unwrap().path[0].index.get(), 1);
        view.handle_key(
            KeyCode::Char('['),
            &snapshot,
            &root,
            &bodies,
            &TraceSnapshot::default(),
            &LogSnapshot::default(),
        );
        assert_eq!(view.pass(&snapshot).unwrap().path[0].index.get(), 0);
        assert_eq!(view.pass(&snapshot).unwrap().stream_invocation, Some(10));
        assert_eq!(view.layout(&root, &bodies).nodes(), before_nodes);
        assert_eq!(view.layout(&root, &bodies).routes(), before_routes);
        assert_eq!(view.layout(&root, &bodies).crossings(), before_crossings);
        view.handle_key(
            KeyCode::Char(']'),
            &snapshot,
            &root,
            &bodies,
            &TraceSnapshot::default(),
            &LogSnapshot::default(),
        );
        assert_eq!(view.pass(&snapshot).unwrap().path[0].index.get(), 1);
        assert_eq!(view.pass(&snapshot).unwrap().stream_invocation, Some(11));
        assert_eq!(view.layout(&root, &bodies).routes(), before_routes);
        view.handle_key(
            KeyCode::Char('h'),
            &snapshot,
            &root,
            &bodies,
            &TraceSnapshot::default(),
            &LogSnapshot::default(),
        );
        assert!(view.path.is_empty());
        snapshot.stream = Some(crate::state::StreamSnapshot {
            hidden_invocations: 1000,
            ..Default::default()
        });
        snapshot.loop_overviews[0].completed_passes = 1000;
        let capture = CaptureView {
            stdout_open: false,
            stderr_open: false,
            stderr_tail: String::new(),
            stderr_dropped: 0,
            error: None,
        };
        let text = details_text(snapshot.nodes.first(), None, &snapshot, &capture, "");
        assert!(text.contains("Completed passes: 1000"));
        assert!(text.contains("Older invocation details hidden: 1000"));
    }

    #[test]
    fn stdout_spool_limit_preserves_the_available_prefix() {
        let mut capture = Capture::new().unwrap();
        capture.written = MAX_STDOUT_BYTES - 2;
        capture.store_stdout(&[1, 0, 255, 4]);
        assert_eq!(capture.written, MAX_STDOUT_BYTES);
        assert!(capture.error.as_deref().unwrap().contains("spool limit"));
        capture.spool.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        capture.spool.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, [1, 0]);
    }

    #[test]
    fn stdout_spool_write_failure_is_reported() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("read-only-spool");
        std::fs::write(&path, []).unwrap();
        let mut capture = Capture::new().unwrap();
        capture.spool = File::open(path).unwrap();
        capture.store_stdout(b"result");
        assert!(
            capture
                .error
                .as_deref()
                .unwrap()
                .contains("spool write failed")
        );
    }

    #[test]
    fn capture_worker_drains_a_full_pipe_without_rendering() {
        let child = Command::new("sh")
            .arg("-c")
            .arg("dd if=/dev/zero bs=65536 count=16 2>/dev/null; dd if=/dev/zero bs=65536 count=32 1>&2 2>/dev/null")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut child: Box<dyn ChildWrapper> = Box::new(child);
        let capture = Arc::new(Mutex::new(Capture::new().unwrap()));
        capture.lock().unwrap().attach(&mut child).unwrap();
        let mut worker = CaptureWorker::start(Arc::clone(&capture));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let view = worker.view();
            if child.try_wait().unwrap().is_some() && !view.stdout_open && !view.stderr_open {
                break;
            }
            if Instant::now() >= deadline {
                child.start_kill().unwrap();
                panic!("capture worker did not drain a full stdout pipe");
            }
            thread::sleep(Duration::from_millis(10));
        }
        worker.stop();
        let capture = capture.lock().unwrap();
        assert_eq!(capture.written, 1_048_576);
        assert_eq!(capture.stderr_tail.len(), MAX_STDERR_BYTES);
        assert_eq!(capture.stderr_dropped, MAX_STDERR_BYTES as u64);
        assert!(capture.error.is_none());
    }
}
