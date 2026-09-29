//! Supervision and terminal presentation for one locally launched workflow.

use crate::{
    description::{DescriptionError, describe_executable},
    duration::format_duration_ns,
    graph::{GraphError, GraphLayout, GraphView},
    receiver::{LoopbackReceiver, ReceiverError},
    state::{NodeObservation, StateSnapshot},
};
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use mf_telemetry::identity::RunId;
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
use snafu::Snafu;
use std::{
    collections::VecDeque,
    env,
    fs::File,
    io::{self, IsTerminal, Read, Seek, SeekFrom, Write},
    os::{fd::AsFd, unix::process::ExitStatusExt},
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

pub fn run_executable(path: &Path) -> Result<u8, RunError> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(RunError::TerminalRequired);
    }
    let area = terminal_area().map_err(|source| RunError::Terminal { source })?;
    let description =
        describe_executable(path).map_err(|source| RunError::Description { source })?;
    let layout = GraphLayout::new(&description).map_err(|source| RunError::Graph { source })?;
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
            .env("MF_RUN_ID", run_id.to_string());
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

fn supervise(
    child: &mut ChildGuard,
    capture: &mut CaptureWorker,
    receiver: &mut LoopbackReceiver,
    layout: &GraphLayout,
    terminal: &mut Terminal<CrosstermBackend<io::Stderr>>,
    signal: &SignalGuard,
) -> Result<(ExitStatus, bool, bool), RunError> {
    let started = Instant::now();
    let mut exited: Option<(ExitStatus, Instant)> = None;
    let mut interrupted_at: Option<Instant> = None;
    let mut killed = false;
    let mut capture_forced = false;
    let mut offset = (0u32, 0u32);
    let mut selected = 0usize;
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
                &snapshot,
                &final_view,
                completed_elapsed,
                offset,
                selected,
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
                                KeyCode::Char('q') | KeyCode::Enter | KeyCode::Esc => {
                                    return Ok((status, interrupted_at.is_some(), capture_forced));
                                }
                                KeyCode::Char('c')
                                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    return Ok((status, interrupted_at.is_some(), capture_forced));
                                }
                                _ => {
                                    navigate(
                                        key.code,
                                        &mut offset,
                                        &mut selected,
                                        layout.nodes().len(),
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
                        &snapshot,
                        &final_view,
                        completed_elapsed,
                        offset,
                        selected,
                        Some(status),
                    )?;
                }
            }
        }
        if last_frame.elapsed() >= FRAME_INTERVAL {
            let snapshot = receiver.snapshot();
            let view = capture.view();
            draw(
                terminal,
                layout,
                &snapshot,
                &view,
                started.elapsed(),
                offset,
                selected,
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
                        navigate(key.code, &mut offset, &mut selected, layout.nodes().len());
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
    layout: &GraphLayout,
    snapshot: &StateSnapshot,
    capture: &CaptureView,
    elapsed: Duration,
    offset: (u32, u32),
    selected: usize,
    status: Option<ExitStatus>,
) -> Result<(), RunError> {
    terminal.draw(|frame| {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1), Constraint::Min(1), Constraint::Length(2),
        ]).areas(frame.area());
        let phase = status.map_or_else(|| "Running".to_owned(), |status| format!("Exited: {}", exit_code(status)));
        let workflow = snapshot.workflow_outcome.map_or("Unknown", |outcome| match outcome {
            mf_telemetry::event::Outcome::Succeeded => "Succeeded",
            mf_telemetry::event::Outcome::Failed => "Failed",
        });
        frame.render_widget(Paragraph::new(format!("Workflow: {workflow}  |  Process: {phase}  |  {:.1}s", elapsed.as_secs_f64())), header);
        let [graph, details] = Layout::horizontal([Constraint::Percentage(70), Constraint::Percentage(30)]).areas(body);
        frame.render_widget(GraphView::new(layout).snapshot(snapshot).offset(offset.0, offset.1).elapsed_ns(elapsed.as_nanos().min(u64::MAX as u128) as u64), graph);
        let selected_node = snapshot.nodes.get(selected);
        frame.render_widget(Paragraph::new(details_text(selected_node, snapshot, capture))
            .block(Block::default().title("Details").borders(Borders::ALL))
            .wrap(Wrap { trim: false }), details);
        let lifecycle = &snapshot.lifecycle;
        let controls = if status.is_some() { "q/Enter close" } else { "Ctrl-C interrupt" };
        frame.render_widget(Paragraph::new(format!(
            "Observation: {:?} | gaps: {} | drops: {} | errors: {} | traces: {}\nArrows pan | f origin | Tab/j/k select | {controls}",
            lifecycle.completeness, lifecycle.known_missing_count, lifecycle.local_drops,
            lifecycle.observation_errors + lifecycle.protocol_conflicts, snapshot.traces.observed_spans,
        )), footer);
    }).map_err(|source| RunError::Terminal { source })?;
    Ok(())
}

fn details_text(
    node: Option<&NodeObservation>,
    snapshot: &StateSnapshot,
    capture: &CaptureView,
) -> String {
    let mut details = String::new();
    if let Some(node) = node {
        details.push_str(&format!(
            "{} ({})\nStatus: {:?}\n",
            node.id, node.kind, node.status
        ));
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
