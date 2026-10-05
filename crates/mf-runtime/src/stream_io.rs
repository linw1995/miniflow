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

pub struct StreamStdio {
    input: Option<TextInput>,
    output: File,
}

impl StreamStdio {
    #[cfg(unix)]
    pub fn claim() -> Result<Self, StreamError> {
        use std::os::fd::{AsRawFd, FromRawFd};
        fn duplicate(fd: libc::c_int) -> io::Result<File> {
            let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
            if duplicate < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(unsafe { File::from_raw_fd(duplicate) })
        }
        fn redirect(source: libc::c_int, target: libc::c_int) -> io::Result<()> {
            if unsafe { libc::dup2(source, target) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        let claim = || -> io::Result<Self> {
            io::stdout().flush()?;
            let input = TextInput::new(duplicate(libc::STDIN_FILENO)?);
            let output = duplicate(libc::STDOUT_FILENO)?;
            let null = File::open("/dev/null")?;
            redirect(null.as_raw_fd(), libc::STDIN_FILENO)?;
            redirect(libc::STDERR_FILENO, libc::STDOUT_FILENO)?;
            Ok(Self {
                input: Some(input),
                output,
            })
        };
        claim().context(StdioSnafu)
    }

    #[cfg(not(unix))]
    pub fn claim() -> Result<Self, StreamError> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "stream stdio requires Linux or macOS",
        ))
        .context(StdioSnafu)
    }

    pub fn take_input(&mut self) -> Option<TextInput> {
        self.input.take()
    }

    pub fn write_json(&mut self, value: &impl serde::Serialize) -> Result<(), StreamError> {
        serde_json::to_writer(&mut self.output, value).context(OutputEncodeSnafu)?;
        self.output
            .write_all(b"\n")
            .and_then(|()| self.output.flush())
            .context(OutputWriteSnafu)
    }

    pub fn run(mut self, instance: StreamInstance) -> Result<StreamSummary, StreamError> {
        let cancellation = instance.cancellation();
        while let Ok(Some(delivery)) = instance.receive() {
            let result = write_record(&mut self.output, delivery.output(), &cancellation);
            match result {
                Ok(()) => {
                    if delivery.acknowledge().is_err() {
                        break;
                    }
                }
                Err(error) => {
                    delivery.fail(error);
                    break;
                }
            }
        }
        instance.join()
    }
}

fn write_record(
    file: &mut File,
    output: &crate::StreamOutput,
    cancellation: &StreamCancellation,
) -> Result<(), StreamError> {
    let mut bytes = serde_json::to_vec(&output.outputs).context(OutputEncodeSnafu)?;
    bytes.push(b'\n');
    let mut remaining = bytes.as_slice();
    while !remaining.is_empty() {
        wait_ready(file, true, cancellation).context(OutputWriteSnafu)?;
        if let Some(error) = cancellation.failure() {
            return Err(error);
        }
        let written = match file.write(&remaining[..remaining.len().min(512)]) {
            Ok(0) => {
                return Err(io::Error::from(io::ErrorKind::WriteZero)).context(OutputWriteSnafu);
            }
            Ok(written) => written,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(source) => return Err(source).context(OutputWriteSnafu),
        };
        remaining = &remaining[written..];
    }
    file.flush().context(OutputWriteSnafu)
}
