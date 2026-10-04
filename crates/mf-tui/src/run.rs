//! Supervision and terminal presentation for one locally launched workflow.

use crate::{
    description::{DescriptionError, describe_executable, describe_interface},
    duration::format_duration_ns,
    graph::{GraphError, GraphLayout, GraphView},
    receiver::{LoopbackReceiver, ReceiverError},
    snapshots::HistoryView,
    state::{LoopPassObservation, NodeObservation, StateSnapshot},
};
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use mf_runtime::{InputResource, MAX_WORKFLOW_INPUT_BYTES, WorkflowArguments, WorkflowInputError};
use mf_telemetry::{description::WorkflowDescription, identity::RunId};
use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    sys::signal::Signal,
};
use process_wrap::std::{ChildWrapper, CommandWrap, ProcessGroup};
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use signal_hook::{SigId, consts::SIGINT, flag, low_level::unregister};
use snafu::{ResultExt, Snafu};
use std::{
    collections::{BTreeMap, VecDeque},
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
const FRAME_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Snafu)]
pub enum RunError {
    #[snafu(display("TUI requires terminal stdin and stderr"))]
    TerminalRequired,
    #[snafu(transparent)]
    Inputs { source: WorkflowInputError },
    #[snafu(display("{message}"))]
    Options { message: String },
    #[snafu(display("could not read workflow input file {path:?}: {source}"))]
    InputFile { path: PathBuf, source: io::Error },
    #[snafu(display("could not prepare private workflow arguments: {source}"))]
    ArgumentFile { source: io::Error },
    #[snafu(display("could not serialize workflow arguments: {source}"))]
    ArgumentSerialization { source: serde_json::Error },
    #[snafu(display("could not describe workflow: {source}"))]
    Description { source: DescriptionError },
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
    #[snafu(display("could not supervise workflow: {source}"))]
    Supervise { source: io::Error },
}

#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    pub inputs: Option<String>,
    pub inputs_file: Option<PathBuf>,
    pub stream_input: Option<PathBuf>,
}

struct PreparedLaunch {
    description: WorkflowDescription,
    arguments: Option<tempfile::NamedTempFile>,
    stdin: Option<File>,
}

fn option_error(message: &str) -> RunError {
    OptionsSnafu {
        message: message.to_owned(),
    }
    .build()
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

fn prepare_launch(path: &Path, options: &RunOptions) -> Result<PreparedLaunch, RunError> {
    if options.inputs.is_some() && options.inputs_file.is_some() {
        return Err(option_error(
            "--inputs and --inputs-file are mutually exclusive",
        ));
    }
    if options.stream_input.as_deref() == Some(Path::new("-")) {
        return Err(option_error(
            "--stream-input requires a file path; terminal stdin is reserved for TUI controls",
        ));
    }
    let arguments = if let Some(json) = &options.inputs {
        WorkflowArguments::from_json(json.as_bytes())?
    } else if let Some(path) = &options.inputs_file {
        let mut bytes = Vec::new();
        open_input_file(path)?
            .take(MAX_WORKFLOW_INPUT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .context(InputFileSnafu { path: path.clone() })?;
        WorkflowArguments::from_json(&bytes)?
    } else {
        WorkflowArguments::default()
    };
    let description = describe_executable(path).context(DescriptionSnafu)?;
    let interface = if description.execution.is_some() {
        let interface = describe_interface(path).context(DescriptionSnafu)?;
        interface.validate_for_description(&description)?;
        interface.schema.validate(&arguments)?;
        Some(interface)
    } else {
        if options.inputs.is_some()
            || options.inputs_file.is_some()
            || options.stream_input.is_some()
        {
            return Err(option_error(
                "this runner has no workflow input interface; recompile it to pass startup arguments or input resources",
            ));
        }
        None
    };
    let mut needs_stdin = false;
    if let Some(interface) = &interface {
        for (node, resources) in &interface.schema.resources {
            for resource in resources {
                match resource {
                    InputResource::Stdin => needs_stdin = true,
                    InputResource::Channel => {
                        return Err(option_error(&format!(
                            "node `{node}` requires a host-bound channel resource; TUI launch cannot supply it"
                        )));
                    }
                }
            }
        }
    }
    let stdin = match (needs_stdin, &options.stream_input) {
        (true, Some(path)) => Some(open_input_file(path)?),
        (true, None) => {
            return Err(option_error(
                "workflow declares a stdin source; provide --stream-input <PATH>",
            ));
        }
        (false, Some(_)) => {
            return Err(option_error(
                "--stream-input was supplied but the workflow declares no stdin resource",
            ));
        }
        (false, None) => None,
    };
    let arguments = if !arguments.0.is_empty() {
        let bytes = serde_json::to_vec(&arguments).context(ArgumentSerializationSnafu)?;
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
        stdin,
    })
}

pub fn run_executable(path: &Path) -> Result<u8, RunError> {
    run_executable_with_options(path, &RunOptions::default())
}

pub fn run_executable_with_options(path: &Path, options: &RunOptions) -> Result<u8, RunError> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(RunError::TerminalRequired);
    }
    let area = terminal_area().map_err(|source| RunError::Terminal { source })?;
    let PreparedLaunch {
        description,
        arguments,
        stdin,
    } = prepare_launch(path, options)?;
    let streaming = description.is_streaming();
    let layout = GraphLayout::new(&description).map_err(|source| RunError::Graph { source })?;
    let mut body_layouts = BTreeMap::new();
    for body in &description.loop_bodies {
        let body_layout = GraphLayout::from_loop_body(&description, &body.path)
            .map_err(|source| RunError::Graph { source })?;
        body_layouts.insert(body.path.clone(), body_layout);
    }
    let capture = Arc::new(Mutex::new(
        Capture::new().map_err(|source| RunError::Spool { source })?,
    ));
    let mut capture_worker = None;
    let run_id = RunId::new();
    let mut receiver = LoopbackReceiver::bind(description, run_id)
        .map_err(|source| RunError::Receiver { source })?;
    let signal = SignalGuard::register().map_err(|source| RunError::Terminal { source })?;
    let guard = TerminalGuard::enter().map_err(|source| RunError::Terminal { source })?;
    // The backend's global size lookup can consult redirected stdout without a controlling TTY.
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(io::stderr()),
        TerminalOptions {
            viewport: Viewport::Fixed(area),
        },
    )
    .map_err(|source| RunError::Terminal { source })?;
    let result = (|| {
        let mut command = Command::new(path);
        command
            .stdin(stdin.map_or_else(Stdio::null, Stdio::from))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(arguments) = &arguments {
            command.arg("--inputs-file").arg(arguments.path());
        }
        for (name, _) in env::vars_os() {
            if name.to_string_lossy().starts_with("OTEL_EXPORTER_OTLP_") {
                command.env_remove(name);
            }
        }
        command
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", receiver.endpoint())
            .env("MF_RUN_ID", run_id.to_string())
            .env(
                mf_telemetry::SNAPSHOT_CAPTURE_ENV,
                if streaming { "0" } else { "1" },
            );
        let mut command = CommandWrap::from(command);
        command.wrap(ProcessGroup::leader());
        let child = command.spawn().map_err(|source| RunError::Spawn {
            path: path.to_owned(),
            source,
        })?;
        let mut child = ChildGuard::new(child);
        capture
            .lock()
            .expect("capture lock was not poisoned")
            .attach(&mut child.child)
            .map_err(|source| RunError::Supervise { source })?;
        capture_worker = Some(CaptureWorker::start(Arc::clone(&capture)));
        supervise(
            &mut child,
            capture_worker.as_mut().expect("capture worker was started"),
            &mut receiver,
            &layout,
            &body_layouts,
            &mut terminal,
            &signal,
        )
    })();
    if let Some(worker) = capture_worker.as_mut() {
        worker.stop();
    }
    drop(terminal);
    drop(guard);
    drop(signal);

    let mut capture = capture.lock().expect("capture lock was not poisoned");
    if let Some(error) = capture.error.as_deref() {
        report(&format!("workflow stdout capture incomplete: {error}"));
    }
    if capture.stderr_dropped != 0 {
        report(&format!(
            "workflow stderr history dropped {} bytes",
            capture.stderr_dropped
        ));
    }
    let delivery = capture.deliver();
    if let Err(error) = delivery.as_ref() {
        report(&format!(
            "could not deliver captured workflow stdout: {error}"
        ));
    }
    let (status, interrupted, capture_forced) = result?;
    if interrupted {
        return Ok(130);
    }
    if capture_forced {
        return Ok(1);
    }
    let code = exit_code(status);
    if capture.error.is_some() || delivery.is_err() {
        Ok(if status.success() { 1 } else { code })
    } else {
        Ok(code)
    }
}

fn terminal_area() -> io::Result<Rect> {
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

struct SignalGuard {
    interrupted: Arc<AtomicBool>,
    id: SigId,
}

impl SignalGuard {
    fn register() -> io::Result<Self> {
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

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
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
    stderr_tail: VecDeque<u8>,
    stderr_dropped: u64,
    error: Option<String>,
}

struct CaptureView {
    stdout_open: bool,
    stderr_open: bool,
    stderr_tail: String,
    stderr_dropped: u64,
    error: Option<String>,
}

struct CaptureWorker {
    capture: Arc<Mutex<Capture>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl CaptureWorker {
    fn start(capture: Arc<Mutex<Capture>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let shared_capture = Arc::clone(&capture);
        let shared_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !shared_stop.load(Ordering::Relaxed) {
                let open = {
                    let mut capture = shared_capture
                        .lock()
                        .expect("capture lock was not poisoned");
                    capture.drain();
                    capture.stdout.is_some() || capture.stderr.is_some()
                };
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
        }
    }

    fn view(&self) -> CaptureView {
        let capture = self.capture.lock().expect("capture lock was not poisoned");
        CaptureView {
            stdout_open: capture.stdout.is_some(),
            stderr_open: capture.stderr.is_some(),
            stderr_tail: capture.stderr_display(),
            stderr_dropped: capture.stderr_dropped,
            error: capture.error.clone(),
        }
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
    fn new() -> io::Result<Self> {
        Ok(Self {
            stdout: None,
            stderr: None,
            spool: tempfile::tempfile()?,
            written: 0,
            stderr_tail: VecDeque::new(),
            stderr_dropped: 0,
            error: None,
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
                    self.error
                        .get_or_insert_with(|| format!("stdout read failed: {error}"));
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
        let allowed = (MAX_STDOUT_BYTES - self.written).min(bytes.len() as u64) as usize;
        if let Err(error) = self.spool.write_all(&bytes[..allowed]) {
            self.error = Some(format!("stdout spool write failed: {error}"));
            return;
        }
        self.written += allowed as u64;
        if allowed < bytes.len() {
            self.error = Some(format!(
                "stdout exceeded the {MAX_STDOUT_BYTES}-byte spool limit"
            ));
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

    fn deliver(&mut self) -> io::Result<()> {
        self.spool.seek(SeekFrom::Start(0))?;
        let mut stdout = io::stdout().lock();
        io::copy(&mut self.spool, &mut stdout)?;
        stdout.flush()
    }
}

fn set_nonblocking(fd: impl AsFd) -> io::Result<()> {
    let flags = OFlag::from_bits_truncate(fcntl(&fd, FcntlArg::F_GETFL)?);
    fcntl(&fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
    Ok(())
}

#[derive(Default)]
struct ViewState {
    history: HistoryView,
    path: Vec<String>,
    offset: (u32, u32),
    selected: usize,
    pass_offset: usize,
}

impl ViewState {
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

    fn handle_key(
        &mut self,
        key: KeyCode,
        snapshot: &StateSnapshot,
        root: &GraphLayout,
        bodies: &BTreeMap<Vec<String>, GraphLayout>,
        history_count: usize,
    ) {
        if self.history.handle_key(key, history_count) {
            return;
        }
        match key {
            KeyCode::Char('l') => {
                if let Some(node) = self.layout(root, bodies).nodes().get(self.selected)
                    && node.kind == "workflow.loop"
                {
                    let mut child = self.path.clone();
                    child.push(node.id.clone());
                    if bodies.contains_key(&child) {
                        self.path = child;
                        self.offset = (0, 0);
                        self.selected = 0;
                        self.pass_offset = 0;
                    }
                }
            }
            KeyCode::Char('h') | KeyCode::Esc => {
                if !self.path.is_empty() {
                    self.path.pop();
                    self.offset = (0, 0);
                    self.selected = 0;
                    self.pass_offset = 0;
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
                navigate(key, &mut self.offset, &mut self.selected, nodes);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn supervise(
    child: &mut ChildGuard,
    capture: &mut CaptureWorker,
    receiver: &mut LoopbackReceiver,
    layout: &GraphLayout,
    body_layouts: &BTreeMap<Vec<String>, GraphLayout>,
    terminal: &mut Terminal<CrosstermBackend<io::Stderr>>,
    signal: &SignalGuard,
) -> Result<(ExitStatus, bool, bool), RunError> {
    let started = Instant::now();
    let mut exited: Option<(ExitStatus, Instant)> = None;
    let mut interrupted_at: Option<Instant> = None;
    let mut killed = false;
    let mut capture_forced = false;
    let mut view_state = ViewState::default();
    let mut last_frame = Instant::now() - FRAME_INTERVAL;
    loop {
        let view = capture.view();
        if exited.is_none()
            && let Some(status) = child
                .child
                .try_wait()
                .map_err(|source| RunError::Supervise { source })?
        {
            exited = Some((status, Instant::now()));
        }
        if exited.is_none()
            && signal.interrupted.swap(false, Ordering::Relaxed)
            && interrupted_at.is_none()
        {
            interrupt(child, &mut interrupted_at);
        }
        if view.error.is_some() && !killed {
            capture_forced = exited.is_none();
            let _ = child.child.start_kill();
            killed = true;
        }
        if interrupted_at.is_some_and(|at| at.elapsed() >= INTERRUPT_TIMEOUT) && !killed {
            let _ = child.child.start_kill();
            killed = true;
        }
        if let Some((status, at)) = exited
            && ((!view.stdout_open && !view.stderr_open) || at.elapsed() >= DRAIN_TIMEOUT)
        {
            if view.stdout_open || view.stderr_open {
                let _ = child.child.start_kill();
            }
            capture.stop();
            {
                let mut captured = capture
                    .capture
                    .lock()
                    .expect("capture lock was not poisoned");
                if captured.stdout.is_some() {
                    captured.error.get_or_insert_with(|| {
                        "stdout pipe remained open after the drain deadline".into()
                    });
                }
                captured.stdout = None;
                captured.stderr = None;
            }
            let final_view = capture.view();
            child.running = false;
            let snapshot = receiver.finish();
            let completed_elapsed = started.elapsed();
            draw(
                terminal,
                layout,
                body_layouts,
                &snapshot,
                &final_view,
                completed_elapsed,
                &mut view_state,
                receiver,
                Some(status),
            )?;
            loop {
                if signal.interrupted.swap(false, Ordering::Relaxed) {
                    return Ok((status, interrupted_at.is_some(), capture_forced));
                }
                if event::poll(Duration::from_millis(100))
                    .map_err(|source| RunError::Terminal { source })?
                {
                    match event::read().map_err(|source| RunError::Terminal { source })? {
                        Event::Key(key)
                            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                        {
                            match key.code {
                                KeyCode::Char('q') | KeyCode::Enter => {
                                    return Ok((status, interrupted_at.is_some(), capture_forced));
                                }
                                KeyCode::Esc
                                    if view_state.path.is_empty()
                                        && !view_state.history.visible =>
                                {
                                    return Ok((status, interrupted_at.is_some(), capture_forced));
                                }
                                KeyCode::Char('c')
                                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    return Ok((status, interrupted_at.is_some(), capture_forced));
                                }
                                _ => {
                                    view_state.handle_key(
                                        key.code,
                                        &snapshot,
                                        layout,
                                        body_layouts,
                                        receiver.history_len(),
                                    );
                                }
                            }
                        }
                        Event::Resize(width, height) => {
                            terminal
                                .resize(Rect::new(0, 0, width, height))
                                .map_err(|source| RunError::Terminal { source })?;
                        }
                        _ => continue,
                    }
                    draw(
                        terminal,
                        layout,
                        body_layouts,
                        &snapshot,
                        &final_view,
                        completed_elapsed,
                        &mut view_state,
                        receiver,
                        Some(status),
                    )?;
                }
            }
        }
        if last_frame.elapsed() >= FRAME_INTERVAL {
            let snapshot = receiver.snapshot_shared();
            let view = capture.view();
            draw(
                terminal,
                layout,
                body_layouts,
                &snapshot,
                &view,
                started.elapsed(),
                &mut view_state,
                receiver,
                None,
            )?;
            last_frame = Instant::now();
        }
        if event::poll(Duration::from_millis(20)).map_err(|source| RunError::Terminal { source })? {
            match event::read().map_err(|source| RunError::Terminal { source })? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        if interrupted_at.is_some() {
                            let _ = child.child.start_kill();
                            killed = true;
                        } else {
                            interrupt(child, &mut interrupted_at);
                        }
                    } else {
                        let snapshot = receiver.snapshot_shared();
                        view_state.handle_key(
                            key.code,
                            &snapshot,
                            layout,
                            body_layouts,
                            receiver.history_len(),
                        );
                    }
                    last_frame = Instant::now() - FRAME_INTERVAL;
                }
                Event::Resize(width, height) => {
                    terminal
                        .resize(Rect::new(0, 0, width, height))
                        .map_err(|source| RunError::Terminal { source })?;
                    last_frame = Instant::now() - FRAME_INTERVAL;
                }
                _ => {}
            }
        }
    }
}

fn interrupt(child: &ChildGuard, interrupted_at: &mut Option<Instant>) {
    let _ = child.child.signal(Signal::SIGINT as i32);
    *interrupted_at = Some(Instant::now());
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

#[allow(clippy::too_many_arguments)]
fn draw(
    terminal: &mut Terminal<CrosstermBackend<io::Stderr>>,
    root_layout: &GraphLayout,
    body_layouts: &BTreeMap<Vec<String>, GraphLayout>,
    snapshot: &StateSnapshot,
    capture: &CaptureView,
    elapsed: Duration,
    view_state: &mut ViewState,
    history: &LoopbackReceiver,
    status: Option<ExitStatus>,
) -> Result<(), RunError> {
    if view_state.history.visible {
        terminal
            .draw(|frame| {
                if snapshot.stream.is_some() {
                    frame.render_widget(Paragraph::new("Data history is unavailable for streaming workflows.\nUse the graph to inspect observed invocations and workflow totals.\n\nv/Esc: graph | Ctrl-C: interrupt/close").block(Block::default().title("Streaming observation").borders(Borders::ALL)).wrap(Wrap { trim: false }), frame.area());
                    return;
                }
                let snapshot = history.history_snapshot(
                    view_state.history.selection(),
                    usize::from(frame.area().height.saturating_sub(6)),
                );
                view_state.history.draw(frame, &snapshot);
            })
            .map_err(|source| RunError::Terminal { source })?;
        return Ok(());
    }
    let layout = view_state.layout(root_layout, body_layouts);
    let pass = view_state.pass(snapshot);
    let display_nodes = if view_state.path.is_empty() {
        None
    } else {
        let order = || {
            layout
                .nodes()
                .iter()
                .map(|node| (node.id.as_str(), node.kind.as_str()))
        };
        Some(pass.map_or_else(
            || {
                order()
                    .map(|(id, kind)| NodeObservation::pending(id, kind))
                    .collect()
            },
            |pass| pass.display_nodes(order()),
        ))
    };
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
        let pass_label = pass.map_or(String::new(), |pass| {
            let index = pass.path.last().map_or(0, |entry| entry.index.get());
            match pass.stream_invocation {
                Some(owner) => format!("  |  Invocation: {owner}  |  Pass: {index}"),
                None => format!("  |  Pass: {index}"),
            }
        });
        frame.render_widget(Paragraph::new(format!(
            "Workflow: {workflow}  |  Process: {phase}  |  Scope: {scope}{pass_label}  |  {:.1}s",
            elapsed.as_secs_f64()
        )), header);
        let [graph, details] = Layout::horizontal([Constraint::Percentage(70), Constraint::Percentage(30)]).areas(body);
        let mut graph_view = GraphView::new(layout)
            .offset(view_state.offset.0, view_state.offset.1)
            .elapsed_ns(elapsed.as_nanos().min(u64::MAX as u128) as u64);
        if view_state.path.is_empty() {
            graph_view = graph_view.snapshot(snapshot);
        } else if let Some(nodes) = &display_nodes {
            graph_view = graph_view.nodes(nodes);
        }
        frame.render_widget(graph_view, graph);
        let selected_node = if let Some(nodes) = &display_nodes {
            nodes.get(view_state.selected)
        } else {
            snapshot.nodes.get(view_state.selected)
        };
        frame.render_widget(Paragraph::new(details_text(selected_node, pass, snapshot, capture))
            .block(Block::default().title("Details").borders(Borders::ALL))
            .wrap(Wrap { trim: false }), details);
        let lifecycle = &snapshot.lifecycle;
        let controls = if status.is_some() { "q/Enter close" } else { "Ctrl-C interrupt" };
        frame.render_widget(Paragraph::new(format!(
            "Observation: {:?} | gaps: {} | drops: {} | errors: {} | traces: {}\nv data history | Arrows pan | Tab/j/k select | l enter Loop | h back | [/] pass | {controls}",
            lifecycle.completeness, lifecycle.known_missing_count, lifecycle.local_drops,
            lifecycle.observation_errors + lifecycle.protocol_conflicts, snapshot.traces.observed_spans,
        )), footer);
    }).map_err(|source| RunError::Terminal { source })?;
    Ok(())
}

fn details_text(
    node: Option<&NodeObservation>,
    pass: Option<&LoopPassObservation>,
    snapshot: &StateSnapshot,
    capture: &CaptureView,
) -> String {
    let mut details = String::new();
    let parent_path = pass.map_or(&[][..], |pass| pass.path.as_slice());
    if let Some(node) = node {
        details.push_str(&format!(
            "{} ({})\nStatus: {:?}\n",
            node.id, node.kind, node.status
        ));
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
            details.push_str(&format!(
                "Skipped by {}.{}\n",
                cause.source_node, cause.source_output
            ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use mf_telemetry::{
        Count,
        description::{
            LoopBodyDescription, NodeDescription, WorkflowDescription, WorkflowDescriptionVersion,
        },
        event::{Event as LifecycleKind, LifecycleEvent, LoopPathEntry},
        identity::WorkflowId,
    };

    fn source_runner(
        directory: &Path,
        resource: Option<InputResource>,
        on_describe: &str,
    ) -> PathBuf {
        use std::{fs, os::unix::fs::PermissionsExt};
        let runner = directory.join("runner");
        let graph = serde_json::json!({"version":"2026-10-03", "workflow_id":format!("sha256:{}", "a".repeat(64)),
            "execution":{"mode":"stream", "event_schema_version":4},
            "nodes":[{"id":"source/id", "kind":"third-party.dynamic_source"}], "execution_order":["source/id"],
            "data_edges":[], "control_edges":[], "loop_bodies":[]});
        let mut resources = serde_json::Map::new();
        if let Some(resource) = resource {
            resources.insert("source/id".into(), serde_json::json!([resource]));
        }
        let interface = serde_json::json!({"version":"2026-10-03", "workflow_id":graph["workflow_id"],
            "schema":{"inputs":{"source/id":{"path":{"type":"string", "required":true}}}, "resources":resources}});
        fs::write(&runner, format!("#!/bin/sh\ncase \"$1\" in\n--describe) printf '%s\\n' '{graph}';;\n--describe-interface) {on_describe}\nprintf '%s\\n' '{interface}';;\n*) exit 99;;\nesac\n")).unwrap();
        fs::set_permissions(&runner, fs::Permissions::from_mode(0o700)).unwrap();
        runner
    }

    fn valid_options() -> RunOptions {
        RunOptions {
            inputs: Some(r#"{"source/id":{"path":"data.txt"}}"#.into()),
            ..Default::default()
        }
    }

    #[test]
    fn preflight_validates_interfaces_and_preserves_owned_parameters() {
        use std::{fs, os::unix::fs::PermissionsExt};
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("arguments.json");
        fs::write(&path, valid_options().inputs.unwrap()).unwrap();
        let runner = source_runner(root.path(), None, &format!("rm '{}'", path.display()));
        let prepared = prepare_launch(
            &runner,
            &RunOptions {
                inputs_file: Some(path.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!path.exists());
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
        assert!(prepared.stdin.is_none());
        drop(prepared);
        assert!(!temporary_path.exists());
        let runner = source_runner(root.path(), None, "");
        for input in [
            "{}",
            r#"{"source/id":{"path":42}}"#,
            r#"{"source/id":{"path":"a","path":"b"}}"#,
            r#"{"source/id":{"path":"a","extra":null}}"#,
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
        options.inputs = Some(" ".repeat(MAX_WORKFLOW_INPUT_BYTES + 1));
        assert!(matches!(
            prepare_launch(&runner, &options),
            Err(RunError::Inputs {
                source: WorkflowInputError::TooLarge { .. }
            })
        ));
    }

    #[test]
    fn preflight_routes_only_declared_file_resources() {
        use std::fs;
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data.jsonl");
        fs::write(&data, "1\r\n2").unwrap();
        let runner = source_runner(root.path(), Some(InputResource::Stdin), "");
        assert!(prepare_launch(&runner, &valid_options()).is_err());
        let mut options = valid_options();
        options.stream_input = Some(data.clone());
        let mut prepared = prepare_launch(&runner, &options).unwrap();
        fs::remove_file(&data).unwrap();
        let mut contents = String::new();
        prepared
            .stdin
            .as_mut()
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, "1\r\n2");
        assert!(prepare_launch(&runner, &options).is_err());
        options.stream_input = Some(root.path().to_owned());
        assert!(prepare_launch(&runner, &options).is_err());
        options.stream_input = Some(PathBuf::from("-"));
        assert!(prepare_launch(&runner, &options).is_err());
        options.stream_input = Some(data);
        let runner = source_runner(root.path(), None, "");
        assert!(prepare_launch(&runner, &options).is_err());
        options.stream_input = None;
        let runner = source_runner(root.path(), Some(InputResource::Channel), "");
        assert!(
            prepare_launch(&runner, &options)
                .err()
                .unwrap()
                .to_string()
                .contains("host-bound channel")
        );
    }

    #[test]
    fn loop_views_preserve_navigation_and_compacted_stream_totals() {
        let description = WorkflowDescription {
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
                path: vec!["repeat".into()],
                nodes: vec![NodeDescription {
                    id: "%loop".into(),
                    kind: "%loop".into(),
                }],
                data_edges: vec![],
                control_edges: vec![],
                execution_order: vec!["%loop".into()],
            }],
        };
        let root = GraphLayout::new(&description).unwrap();
        let bodies = BTreeMap::from([(
            vec!["repeat".into()],
            GraphLayout::from_loop_body(&description, &["repeat".into()]).unwrap(),
        )]);
        let run_id = RunId::new();
        let mut session = crate::state::SessionState::new(description.clone(), run_id).unwrap();
        let count = |value| Count::try_from(value).unwrap();
        for (sequence, event) in [
            (
                1,
                LifecycleKind::WorkflowStarted {
                    node_count: count(2),
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
        let snapshot = session.snapshot();
        let mut view = ViewState::default();
        view.handle_key(KeyCode::Char('l'), &snapshot, &root, &bodies, 0);
        assert_eq!(view.path, ["repeat"]);
        assert_eq!(view.layout(&root, &bodies).nodes()[0].id, "%loop");
        assert_eq!(view.pass(&snapshot).unwrap().path[0].index.get(), 1);
        view.handle_key(KeyCode::Char('['), &snapshot, &root, &bodies, 0);
        assert_eq!(view.pass(&snapshot).unwrap().path[0].index.get(), 0);
        view.handle_key(KeyCode::Char(']'), &snapshot, &root, &bodies, 0);
        assert_eq!(view.pass(&snapshot).unwrap().path[0].index.get(), 1);
        view.handle_key(KeyCode::Char('h'), &snapshot, &root, &bodies, 0);
        assert!(view.path.is_empty());
        let mut snapshot = snapshot;
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
        let text = details_text(snapshot.nodes.first(), None, &snapshot, &capture);
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
