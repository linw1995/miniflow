//! Bounded preflight for one locally launched compiled workflow.

use mf_runtime::{WorkflowInputError, WorkflowInterface};
use mf_telemetry::{
    ContractError,
    description::{MAX_DESCRIPTION_BYTES, WorkflowDescription},
};
#[cfg(windows)]
use process_wrap::std::JobObject;
#[cfg(unix)]
use process_wrap::std::ProcessGroup;
use process_wrap::std::{ChildWrapper, CommandWrap};
use snafu::Snafu;
use std::{
    collections::VecDeque,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub const DESCRIPTION_TIMEOUT: Duration = Duration::from_secs(30);
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
pub const MAX_DIAGNOSTIC_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct DescriptionLimits {
    pub timeout: Duration,
    pub drain_timeout: Duration,
    pub max_bytes: usize,
    pub max_diagnostic_bytes: usize,
}

impl Default for DescriptionLimits {
    fn default() -> Self {
        Self {
            timeout: DESCRIPTION_TIMEOUT,
            drain_timeout: DRAIN_TIMEOUT,
            max_bytes: MAX_DESCRIPTION_BYTES + 1,
            max_diagnostic_bytes: MAX_DIAGNOSTIC_BYTES,
        }
    }
}

#[derive(Debug, Snafu)]
pub enum DescriptionError {
    #[snafu(display(
        "streaming event schema {schema_version} is unsupported by TUI; recompile the workflow for schema 4"
    ))]
    UnsupportedStream { schema_version: i64 },
    #[snafu(display("runner returned an invalid interface: {source}; recompile the workflow"))]
    InvalidInterface { source: WorkflowInputError },
    #[snafu(display("could not start workflow description from {path:?}: {source}"))]
    Spawn { path: PathBuf, source: io::Error },
    #[snafu(display("could not read workflow description: {source}"))]
    Read { source: io::Error },
    #[snafu(display("workflow description exceeded the {limit}-byte limit"))]
    TooLarge { limit: usize },
    #[snafu(display("workflow description timed out after {timeout:?}"))]
    Timeout { timeout: Duration },
    #[snafu(display("workflow description output did not close after process exit"))]
    OpenPipe,
    #[snafu(display("workflow description reader stopped unexpectedly"))]
    ReaderPanic,
    #[snafu(display(
        "workflow description exited with {status}: {diagnostics}; recompile older workflow executables"
    ))]
    Exit {
        status: ExitStatus,
        diagnostics: String,
    },
    #[snafu(display("runner returned an invalid description: {source}; recompile the workflow"))]
    Invalid { source: ContractError },
    #[snafu(display("runner omitted the description terminator; recompile the workflow"))]
    MissingTerminator,
}

pub fn describe_executable(path: &Path) -> Result<WorkflowDescription, DescriptionError> {
    describe_executable_with_limits(path, DescriptionLimits::default())
}

pub fn describe_executable_with_limits(
    path: &Path,
    limits: DescriptionLimits,
) -> Result<WorkflowDescription, DescriptionError> {
    let json = inspect_executable(path, "--describe", limits)?;
    let description = WorkflowDescription::from_json(&json)
        .map_err(|source| DescriptionError::Invalid { source })?;
    if description.is_streaming()
        && description.event_schema_version() != mf_telemetry::STREAM_EVENT_SCHEMA_VERSION
    {
        return Err(DescriptionError::UnsupportedStream {
            schema_version: description.event_schema_version(),
        });
    }
    Ok(description)
}

pub fn describe_interface(path: &Path) -> Result<WorkflowInterface, DescriptionError> {
    describe_interface_with_limits(path, DescriptionLimits::default())
}

pub fn describe_interface_with_limits(
    path: &Path,
    limits: DescriptionLimits,
) -> Result<WorkflowInterface, DescriptionError> {
    let json = inspect_executable(path, "--describe-interface", limits)?;
    WorkflowInterface::from_json(&json)
        .map_err(|source| DescriptionError::InvalidInterface { source })
}

fn inspect_executable(
    path: &Path,
    flag: &str,
    limits: DescriptionLimits,
) -> Result<Vec<u8>, DescriptionError> {
    let limits = DescriptionLimits {
        timeout: limits.timeout.min(DESCRIPTION_TIMEOUT),
        drain_timeout: limits.drain_timeout.min(DRAIN_TIMEOUT),
        max_bytes: limits.max_bytes.min(MAX_DESCRIPTION_BYTES + 1),
        max_diagnostic_bytes: limits.max_diagnostic_bytes.min(MAX_DIAGNOSTIC_BYTES),
    };
    let mut command = Command::new(path);
    command
        .arg(flag)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut command = CommandWrap::from(command);
    #[cfg(unix)]
    command.wrap(ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(JobObject);
    let child = command.spawn().map_err(|source| DescriptionError::Spawn {
        path: path.to_owned(),
        source,
    })?;
    let mut child = ChildGuard::new(child);
    let stdout = child.child.stdout().take().expect("stdout was piped");
    let stderr = child.child.stderr().take().expect("stderr was piped");
    let (sender, receiver) = mpsc::channel();
    let output_sender = sender.clone();
    let output_reader = thread::spawn(move || {
        let _ = output_sender.send(ReadResult::Output(read_output(stdout, limits.max_bytes)));
    });
    let diagnostic_reader = thread::spawn(move || {
        let _ = sender.send(ReadResult::Diagnostics(read_diagnostics(
            stderr,
            limits.max_diagnostic_bytes,
        )));
    });

    let started = Instant::now();
    let mut exited_at = None;
    let mut output = None;
    let mut diagnostics = None;
    let status = loop {
        while let Ok(result) = receiver.try_recv() {
            match result {
                ReadResult::Output(Ok(bytes)) => output = Some(bytes),
                ReadResult::Output(Err(OutputReadError::TooLarge)) => {
                    return Err(DescriptionError::TooLarge {
                        limit: limits.max_bytes,
                    });
                }
                ReadResult::Output(Err(OutputReadError::Io(source)))
                | ReadResult::Diagnostics(Err(source)) => {
                    return Err(DescriptionError::Read { source });
                }
                ReadResult::Diagnostics(Ok(tail)) => diagnostics = Some(tail),
            }
        }
        let status = child
            .child
            .try_wait()
            .map_err(|source| DescriptionError::Read { source })?;
        if status.is_some() {
            exited_at.get_or_insert_with(Instant::now);
        }
        if output.is_some()
            && diagnostics.is_some()
            && let Some(status) = status
        {
            child.disarm();
            break status;
        }
        let deadline = exited_at.map_or(started + limits.timeout, |at| at + limits.drain_timeout);
        if Instant::now() >= deadline {
            if status.is_some() {
                return Err(DescriptionError::OpenPipe);
            }
            return Err(DescriptionError::Timeout {
                timeout: limits.timeout,
            });
        }
        thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(10)),
        );
    };
    output_reader
        .join()
        .map_err(|_| DescriptionError::ReaderPanic)?;
    diagnostic_reader
        .join()
        .map_err(|_| DescriptionError::ReaderPanic)?;
    let details = diagnostics.expect("reader completed").display();
    if !status.success() {
        return Err(DescriptionError::Exit {
            status,
            diagnostics: details,
        });
    }
    let bytes = output.expect("reader completed");
    let json = bytes
        .strip_suffix(b"\n")
        .ok_or(DescriptionError::MissingTerminator)?;
    Ok(json.to_vec())
}

enum ReadResult {
    Output(Result<Vec<u8>, OutputReadError>),
    Diagnostics(io::Result<DiagnosticTail>),
}

enum OutputReadError {
    TooLarge,
    Io(io::Error),
}

fn read_output(mut stream: impl Read, limit: usize) -> Result<Vec<u8>, OutputReadError> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(output),
            Ok(size) if size > limit.saturating_sub(output.len()) => {
                return Err(OutputReadError::TooLarge);
            }
            Ok(size) => output.extend_from_slice(&buffer[..size]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(OutputReadError::Io(error)),
        }
    }
}

fn read_diagnostics(mut stream: impl Read, limit: usize) -> io::Result<DiagnosticTail> {
    let mut tail = DiagnosticTail::new(limit);
    let mut buffer = [0u8; 16 * 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(tail),
            Ok(size) => tail.push(&buffer[..size]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

struct DiagnosticTail {
    bytes: VecDeque<u8>,
    dropped: usize,
    limit: usize,
}

impl DiagnosticTail {
    fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            dropped: 0,
            limit,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= self.limit {
            self.dropped = self
                .dropped
                .saturating_add(self.bytes.len() + bytes.len() - self.limit);
            self.bytes.clear();
            self.bytes
                .extend(bytes[bytes.len() - self.limit..].iter().copied());
        } else {
            let remove = self
                .bytes
                .len()
                .saturating_add(bytes.len())
                .saturating_sub(self.limit);
            for _ in 0..remove {
                self.bytes.pop_front();
            }
            self.dropped = self.dropped.saturating_add(remove);
            self.bytes.extend(bytes.iter().copied());
        }
    }

    fn display(&self) -> String {
        let mut details =
            String::from_utf8_lossy(&self.bytes.iter().copied().collect::<Vec<_>>()).into_owned();
        details.retain(|c| !c.is_control() || c == '\n' || c == '\t');
        if self.dropped != 0 {
            details.push_str(&format!(" [dropped {} diagnostic bytes]", self.dropped));
        }
        details
    }
}

struct ChildGuard {
    child: Box<dyn ChildWrapper>,
    needs_abort: bool,
}

impl ChildGuard {
    fn new(child: Box<dyn ChildWrapper>) -> Self {
        Self {
            child,
            needs_abort: true,
        }
    }

    fn disarm(&mut self) {
        self.needs_abort = false;
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.needs_abort {
            let _ = self.child.start_kill();
        }
        let _ = self.child.wait();
    }
}
