use crate::{StreamCancellation, StreamError, StreamInstance, StreamSummary, ValueRef, ValueType};
use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Read, Write},
};

#[derive(Debug)]
pub struct StreamInput {
    file: File,
    buffered: VecDeque<u8>,
    partial: Vec<u8>,
    line: u64,
    eof: bool,
}

impl StreamInput {
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
        reserve().map_err(|source| StreamError::Stdio {
            source: source.into(),
        })
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

    pub fn next_value(
        &mut self,
        value_type: &ValueType,
        cancellation: &StreamCancellation,
    ) -> Result<Option<ValueRef>, StreamError> {
        loop {
            if let Some(error) = cancellation.failure() {
                return Err(error);
            }
            while let Some(byte) = self.buffered.pop_front() {
                if byte == b'\n' {
                    return self.finish_record(value_type).map(Some);
                }
                self.partial.push(byte);
            }
            if self.eof {
                return if self.partial.is_empty() {
                    Ok(None)
                } else {
                    self.finish_record(value_type).map(Some)
                };
            }
            wait_ready(&self.file, false, cancellation)
                .map_err(|error| input_error(self.line, error))?;
            if let Some(error) = cancellation.failure() {
                return Err(error);
            }
            let mut bytes = [0; 8192];
            match self.file.read(&mut bytes) {
                Ok(0) => self.eof = true,
                Ok(len) => self.buffered.extend(&bytes[..len]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(input_error(self.line, error)),
            }
        }
    }

    fn finish_record(&mut self, value_type: &ValueType) -> Result<ValueRef, StreamError> {
        let line = self.line;
        self.line = line.checked_add(1).ok_or_else(|| StreamError::Resource {
            message: "input line counter exhausted".into(),
        })?;
        let mut record = std::mem::take(&mut self.partial);
        if record.last() == Some(&b'\r') {
            record.pop();
        }
        if record.iter().all(u8::is_ascii_whitespace) {
            return Err(StreamError::InputFailure {
                message: format!("line {line}: blank JSON Lines record"),
            });
        }
        let value: ValueRef = serde_json::from_slice::<serde_json::Value>(&record)
            .map_err(|error| input_error(line, error))?
            .into();
        value_type
            .validate_shared(&value)
            .map_err(|error| input_error(line, error))?;
        Ok(value)
    }
}

fn input_error(line: u64, error: impl std::error::Error + Send + Sync + 'static) -> StreamError {
    StreamError::InputRecord {
        line,
        source: std::sync::Arc::new(error),
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
    input: Option<StreamInput>,
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
            let input = StreamInput::new(duplicate(libc::STDIN_FILENO)?);
            let output = duplicate(libc::STDOUT_FILENO)?;
            let null = File::open("/dev/null")?;
            redirect(null.as_raw_fd(), libc::STDIN_FILENO)?;
            redirect(libc::STDERR_FILENO, libc::STDOUT_FILENO)?;
            Ok(Self {
                input: Some(input),
                output,
            })
        };
        claim().map_err(|source| StreamError::Stdio {
            source: source.into(),
        })
    }

    #[cfg(not(unix))]
    pub fn claim() -> Result<Self, StreamError> {
        Err(StreamError::Preparation {
            message: "stream stdio requires Linux or macOS".into(),
        })
    }

    pub fn take_input(&mut self) -> Option<StreamInput> {
        self.input.take()
    }

    pub fn write_json(&mut self, value: &impl serde::Serialize) -> Result<(), StreamError> {
        serde_json::to_writer(&mut self.output, value).map_err(|source| {
            StreamError::OutputEncode {
                source: source.into(),
            }
        })?;
        self.output
            .write_all(b"\n")
            .and_then(|()| self.output.flush())
            .map_err(|source| StreamError::OutputWrite {
                source: source.into(),
            })
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
    let mut bytes =
        serde_json::to_vec(&output.outputs).map_err(|source| StreamError::OutputEncode {
            source: source.into(),
        })?;
    bytes.push(b'\n');
    let mut remaining = bytes.as_slice();
    while !remaining.is_empty() {
        wait_ready(file, true, cancellation).map_err(|source| StreamError::OutputWrite {
            source: source.into(),
        })?;
        if let Some(error) = cancellation.failure() {
            return Err(error);
        }
        let written = match file.write(&remaining[..remaining.len().min(512)]) {
            Ok(0) => {
                return Err(StreamError::OutputWrite {
                    source: io::Error::from(io::ErrorKind::WriteZero).into(),
                });
            }
            Ok(written) => written,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(source) => {
                return Err(StreamError::OutputWrite {
                    source: source.into(),
                });
            }
        };
        remaining = &remaining[written..];
    }
    file.flush().map_err(|source| StreamError::OutputWrite {
        source: source.into(),
    })
}
