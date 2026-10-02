use crate::{StreamError, StreamInstance, StreamSummary};

#[cfg(unix)]
mod unix {
    use super::*;
    use crate::stream_instance::{
        InputFailureSnafu, InputRecordSnafu, OutputEncodeSnafu, OutputWriteSnafu, StdioSnafu,
        ThreadSpawnSnafu,
    };
    use crate::{StreamSender, ValueRef, ValueType};
    use snafu::{IntoError, ResultExt};
    use std::{
        fs::File,
        io::{self, Read, Write},
        os::fd::{AsRawFd, FromRawFd},
        panic::{AssertUnwindSafe, catch_unwind},
        thread,
    };

    /// Claims process stdio before plugin construction or worker startup.
    pub struct StreamStdio {
        input: File,
        output: File,
    }

    fn duplicate(fd: libc::c_int) -> io::Result<File> {
        // Plugin subprocesses must not inherit the private protocol descriptors.
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

    impl StreamStdio {
        pub fn claim() -> Result<Self, StreamError> {
            let claim = || -> io::Result<Self> {
                io::stdout().flush()?;
                let input = duplicate(libc::STDIN_FILENO)?;
                let output = duplicate(libc::STDOUT_FILENO)?;
                let null = File::open("/dev/null")?;
                redirect(null.as_raw_fd(), libc::STDIN_FILENO)?;
                redirect(libc::STDERR_FILENO, libc::STDOUT_FILENO)?;
                Ok(Self { input, output })
            };
            claim().context(StdioSnafu)
        }

        pub fn run(mut self, instance: StreamInstance) -> Result<StreamSummary, StreamError> {
            let input = instance.input();
            let reader_input = input.clone();
            let value_type = instance.execution().input_type.clone();
            let reader = thread::Builder::new()
                .name("workflow-input".into())
                .spawn(move || {
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        read_lines(self.input, &reader_input, &value_type)
                    }));
                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => reader_input.fail(error),
                        Err(_) => reader_input.fail(
                            InputFailureSnafu {
                                message: "input reader panicked",
                            }
                            .build(),
                        ),
                    }
                })
                .context(ThreadSpawnSnafu {
                    thread: "workflow-input",
                })
                .inspect_err(|error| {
                    input.fail(error.clone());
                })?;
            loop {
                let delivery = match instance.receive() {
                    Ok(Some(delivery)) => delivery,
                    Ok(None) | Err(_) => break,
                };
                let result = write_record(&mut self.output, delivery.output(), &input);
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
            let result = instance.join();
            if let Err(error) = &result {
                input.fail(error.clone());
            }
            if reader.join().is_err() {
                return result.and(
                    InputFailureSnafu {
                        message: "input reader stopped unexpectedly",
                    }
                    .fail(),
                );
            }
            result
        }
    }

    fn wait_ready(file: &File, events: libc::c_short, input: &StreamSender) -> io::Result<bool> {
        loop {
            if input.failure().is_some() {
                return Ok(false);
            }
            let mut descriptor = libc::pollfd {
                fd: file.as_raw_fd(),
                events,
                revents: 0,
            };
            // Polling keeps idle input and blocked output interruptible by instance failure.
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
            if events == libc::POLLOUT && descriptor.revents & (libc::POLLERR | libc::POLLHUP) != 0
            {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            if descriptor.revents & (events | libc::POLLERR | libc::POLLHUP) != 0 {
                return Ok(true);
            }
        }
    }

    fn read_lines(
        mut file: File,
        input: &StreamSender,
        value_type: &ValueType,
    ) -> Result<(), StreamError> {
        let mut chunk = [0; 8192];
        let mut record = Vec::new();
        let mut line = 1u64;
        loop {
            if !wait_ready(&file, libc::POLLIN, input)
                .boxed()
                .context(InputRecordSnafu { line })?
            {
                return Err(input.failure().expect("instance failure stops I/O"));
            }
            let read = match file.read(&mut chunk) {
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(InputRecordSnafu { line }.into_error(Box::new(error))),
            };
            if read == 0 {
                if !record.is_empty() {
                    accept_record(&mut record, line, input, value_type)?;
                }
                input.close();
                return Ok(());
            }
            for &byte in &chunk[..read] {
                if byte == b'\n' {
                    accept_record(&mut record, line, input, value_type)?;
                    line = line
                        .checked_add(1)
                        .ok_or_else(|| input_error(line, "line counter exhausted"))?;
                } else {
                    record.push(byte);
                }
            }
        }
    }

    fn accept_record(
        record: &mut Vec<u8>,
        line: u64,
        input: &StreamSender,
        value_type: &ValueType,
    ) -> Result<(), StreamError> {
        if record.last() == Some(&b'\r') {
            record.pop();
        }
        if record.iter().all(u8::is_ascii_whitespace) {
            return Err(input_error(line, "blank JSON Lines record"));
        }
        let value: ValueRef = serde_json::from_slice::<serde_json::Value>(record)
            .boxed()
            .context(InputRecordSnafu { line })?
            .into();
        value_type
            .validate_shared(&value)
            .boxed()
            .context(InputRecordSnafu { line })?;
        input.send(value)?;
        record.clear();
        Ok(())
    }

    fn input_error(line: u64, message: &str) -> StreamError {
        InputFailureSnafu {
            message: format!("line {line}: {message}"),
        }
        .build()
    }

    fn write_record(
        file: &mut File,
        output: &crate::StreamOutput,
        input: &StreamSender,
    ) -> Result<(), StreamError> {
        let mut bytes = serde_json::to_vec(&output.outputs).context(OutputEncodeSnafu)?;
        bytes.push(b'\n');
        let mut remaining = bytes.as_slice();
        while !remaining.is_empty() {
            if !wait_ready(file, libc::POLLOUT, input).context(OutputWriteSnafu)? {
                return Err(input.failure().expect("instance failure stops I/O"));
            }
            // A single writer uses at most the POSIX minimum PIPE_BUF after polling.
            let written = match file.write(&remaining[..remaining.len().min(512)]) {
                Ok(0) => {
                    return Err(
                        OutputWriteSnafu.into_error(io::Error::from(io::ErrorKind::WriteZero))
                    );
                }
                Ok(written) => written,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(OutputWriteSnafu.into_error(error)),
            };
            remaining = &remaining[written..];
        }
        file.flush().context(OutputWriteSnafu)
    }
}

#[cfg(unix)]
pub use unix::StreamStdio;

#[cfg(not(unix))]
pub struct StreamStdio {
    _private: (),
}
#[cfg(not(unix))]
impl StreamStdio {
    pub fn claim() -> Result<Self, StreamError> {
        Err(StreamError::Preparation {
            message: "stream stdio requires Linux or macOS".into(),
        })
    }
    pub fn run(self, instance: StreamInstance) -> Result<StreamSummary, StreamError> {
        let error = StreamError::Preparation {
            message: "stream stdio requires Linux or macOS".into(),
        };
        instance.input().fail(error.clone());
        let _ = instance.join();
        Err(error)
    }
}
