use crate::stream_instance::{
    InputRecordSnafu, OutputEncodeSnafu, OutputWriteSnafu, ResourceSnafu, StdioSnafu,
};
use crate::{StreamCancellation, StreamError, StreamInstance, StreamSummary};
use snafu::ResultExt;
use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Read, Write},
};

#[derive(Debug)]
pub struct TextInput {
    file: File,
    buffered: VecDeque<u8>,
    partial: Vec<u8>,
    line: u64,
    eof: bool,
}

impl TextInput {
    #[cfg(unix)]
    pub fn claim() -> Result<Self, StreamError> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let reserve = || -> io::Result<Self> {
            let fd = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let input = unsafe { File::from_raw_fd(fd) };
            let null = File::open("/dev/null")?;
            if unsafe { libc::dup2(null.as_raw_fd(), libc::STDIN_FILENO) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self::new(input))
        };
        reserve().context(StdioSnafu)
    }

    pub fn new(file: File) -> Self {
        Self {
            file,
            buffered: VecDeque::new(),
            partial: Vec::new(),
            line: 1,
            eof: false,
        }
    }

    pub fn next_line(
        &mut self,
        cancellation: &StreamCancellation,
    ) -> Result<Option<String>, StreamError> {
        loop {
            if let Some(error) = cancellation.failure() {
                return Err(error);
            }
            while let Some(byte) = self.buffered.pop_front() {
                if byte == b'\n' {
                    return self.finish_line(true).map(Some);
                }
                self.partial.push(byte);
            }
            if self.eof {
                return if self.partial.is_empty() {
                    Ok(None)
                } else {
                    self.finish_line(false).map(Some)
                };
            }
            wait_ready(&self.file, false, cancellation)
                .map_err(Box::<dyn std::error::Error + Send + Sync>::from)
                .context(InputRecordSnafu { line: self.line })?;
            if let Some(error) = cancellation.failure() {
                return Err(error);
            }
            let mut bytes = [0; 8192];
            match self.file.read(&mut bytes) {
                Ok(0) => self.eof = true,
                Ok(len) => self.buffered.extend(&bytes[..len]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    return Err(Box::<dyn std::error::Error + Send + Sync>::from(error))
                        .context(InputRecordSnafu { line: self.line });
                }
            }
        }
    }

    fn finish_line(&mut self, terminated: bool) -> Result<String, StreamError> {
        let line = self.line;
        self.line = line.checked_add(1).ok_or_else(|| {
            ResourceSnafu {
                message: String::from("input line counter exhausted"),
            }
            .build()
        })?;
        let mut record = std::mem::take(&mut self.partial);
        if terminated && record.last() == Some(&b'\r') {
            record.pop();
        }
        String::from_utf8(record)
            .map_err(Box::<dyn std::error::Error + Send + Sync>::from)
            .context(InputRecordSnafu { line })
    }
}

#[cfg(unix)]
fn wait_ready(file: &File, writing: bool, cancellation: &StreamCancellation) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let events = if writing { libc::POLLOUT } else { libc::POLLIN };
    loop {
        if cancellation.failure().is_some() {
            return Ok(());
        }
        let mut descriptor = libc::pollfd {
            fd: file.as_raw_fd(),
            events,
            revents: 0,
        };
        // The deadline lets cancellation interrupt idle input and blocked output.
        let ready = unsafe { libc::poll(&mut descriptor, 1, 50) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready == 0 {
            continue;
        }
        if descriptor.revents & libc::POLLNVAL != 0 {
            // Darwin reports POLLNVAL for valid character devices such as /dev/null.
            #[cfg(target_os = "macos")]
            if writing
                && std::os::unix::fs::FileTypeExt::is_char_device(&file.metadata()?.file_type())
            {
                return Ok(());
            }
            return Err(io::Error::other("stream descriptor is invalid"));
        }
        if writing && descriptor.revents & (libc::POLLERR | libc::POLLHUP) != 0 {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        if descriptor.revents & (events | libc::POLLERR | libc::POLLHUP) != 0 {
            return Ok(());
        }
    }
}

#[cfg(not(unix))]
fn wait_ready(_: &File, _: bool, _: &StreamCancellation) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "stream stdio requires Linux or macOS",
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLevel {
    Warning,
    Error,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerDiagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
}

/// Reserves the runner's standard streams even when an invocation routes business output elsewhere.
pub struct RunnerStdio {
    output: File,
    #[cfg(unix)]
    rpc: Option<crate::callback_rpc::Client>,
}

impl RunnerStdio {
    #[cfg(unix)]
    pub fn claim() -> Result<Self, StreamError> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let reserve = || -> io::Result<Self> {
            io::stdout().flush()?;
            let fd = unsafe { libc::fcntl(libc::STDOUT_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let output = unsafe { File::from_raw_fd(fd) };
            let null = File::open("/dev/null")?;
            if unsafe { libc::dup2(null.as_raw_fd(), libc::STDIN_FILENO) } < 0
                || unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { output, rpc: None })
        };
        reserve().context(StdioSnafu)
    }

    #[cfg(not(unix))]
    pub fn claim() -> Result<Self, StreamError> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "runner stdio requires Linux or macOS",
        ))
        .context(StdioSnafu)
    }

    #[cfg(unix)]
    pub fn attach_host(
        &mut self,
        sites: &crate::callback::CallbackSites,
        cancellation: &StreamCancellation,
    ) -> Result<
        (
            crate::callback::HostCallbacks,
            Option<crate::WorkflowArguments>,
        ),
        crate::callback::CallbackError,
    > {
        self.rpc = crate::callback_rpc::from_env(sites, cancellation)?;
        let Some(client) = &mut self.rpc else {
            return Ok(Default::default());
        };
        Ok((client.callbacks(), client.take_inputs()?))
    }

    pub fn diagnostic(
        &self,
        level: DiagnosticLevel,
        message: std::fmt::Arguments<'_>,
    ) -> Result<(), crate::callback::CallbackError> {
        #[cfg(unix)]
        if let Some(client) = &self.rpc
            && client.io().diagnostics
        {
            return client.diagnostic(RunnerDiagnostic {
                level,
                message: message.to_string(),
            });
        }
        let _ = level;
        eprintln!("{message}");
        Ok(())
    }

    pub fn warn(&self, message: std::fmt::Arguments<'_>) {
        // Observation warnings do not replace a workflow failure. A failed RPC write poisons the invocation.
        let _ = self.diagnostic(DiagnosticLevel::Warning, message);
    }

    pub fn finish(self) -> Result<(), crate::callback::CallbackError> {
        #[cfg(unix)]
        if let Some(client) = self.rpc {
            client.finish()?;
        }
        Ok(())
    }

    pub fn write_output(
        &self,
        values: crate::NodeValues,
        cancellation: &StreamCancellation,
    ) -> Result<(), StreamError> {
        #[cfg(unix)]
        if let Some(client) = &self.rpc
            && client.io().output
        {
            return client
                .output(values, cancellation)
                .context(crate::stream_instance::OutputRpcSnafu);
        }
        let mut bytes = serde_json::to_vec(&values).context(OutputEncodeSnafu)?;
        bytes.push(b'\n');
        let mut remaining = bytes.as_slice();
        let mut file = &self.output;
        while !remaining.is_empty() {
            wait_ready(file, true, cancellation).context(OutputWriteSnafu)?;
            if let Some(error) = cancellation.failure() {
                return Err(error);
            }
            let written = match file.write(&remaining[..remaining.len().min(512)]) {
                Ok(0) => {
                    return Err(io::Error::from(io::ErrorKind::WriteZero))
                        .context(OutputWriteSnafu);
                }
                Ok(written) => written,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(source) => return Err(source).context(OutputWriteSnafu),
            };
            remaining = &remaining[written..];
        }
        file.flush().context(OutputWriteSnafu)
    }

    pub fn run(&self, instance: StreamInstance) -> Result<StreamSummary, StreamError> {
        let cancellation = instance.cancellation();
        while let Ok(Some(delivery)) = instance.receive() {
            match self.write_output(delivery.output().outputs.clone(), &cancellation) {
                Ok(()) => {
                    if delivery.acknowledge().is_err() {
                        break;
                    }
                }
                Err(error) => {
                    delivery.fail(error.clone());
                    let _ = instance.join();
                    return Err(error);
                }
            }
        }
        instance.join()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn output_supports_dev_null_but_preserves_cancellation_and_write_failures() {
        let output = RunnerStdio {
            rpc: None,
            output: File::options().write(true).open("/dev/null").unwrap(),
        };
        let cancel = StreamCancellation::default();
        output.write_output(Default::default(), &cancel).unwrap();
        cancel.cancel(
            crate::StreamExecutionSnafu {
                message: "cancelled before output",
            }
            .build(),
        );
        assert!(matches!(
            output.write_output(Default::default(), &cancel),
            Err(StreamError::Execution { .. })
        ));

        let file = tempfile::NamedTempFile::new().unwrap();
        let read_only = RunnerStdio {
            rpc: None,
            output: File::open(file.path()).unwrap(),
        };
        assert!(matches!(
            read_only.write_output(Default::default(), &Default::default()),
            Err(StreamError::OutputWrite { .. })
        ));
        assert_eq!(file.as_file().metadata().unwrap().len(), 0);
    }
}
