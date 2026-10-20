use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeMetadata, NodeRegistration,
    NodeValue, PreparedNode, TypedNodeResult, TypedTaskNode, ValueRef, deserialize_config,
};
use serde::Deserialize;
use snafu::{ResultExt, Snafu, ensure};
use std::{
    io::{Read, Seek, SeekFrom},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const MAX_INPUT_BYTES: u64 = 1024 * 1024;
const TERMINATION_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("invalid process configuration: {message}"))]
    Invalid { message: &'static str },
    #[snafu(display("{action}: {source}"))]
    Io {
        action: &'static str,
        source: std::io::Error,
    },
    #[snafu(display("process JSON transport failed: {source}"))]
    Json { source: serde_json::Error },
    #[snafu(display("process stdout is not UTF-8: {source}"))]
    Encoding { source: std::string::FromUtf8Error },
    #[snafu(display("process exited with {status}"))]
    Exit { status: ExitStatus },
    #[snafu(display("process execution timed out"))]
    Timeout,
    #[snafu(display("process {stream} exceeded its byte limit"))]
    Limit { stream: &'static str },
    #[snafu(transparent)]
    Cancelled { source: mf_runtime::StreamError },
}

impl From<Error> for NodeBuildError {
    fn from(source: Error) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}
impl From<Error> for NodeExecutionError {
    fn from(source: Error) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OutputFormat {
    #[default]
    Text,
    Json,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    output: OutputFormat,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
    #[serde(default = "default_output_limit")]
    max_stdout_bytes: u64,
}
fn default_timeout() -> u64 {
    3_600_000
}
fn default_output_limit() -> u64 {
    1024 * 1024
}

#[derive(NodeValue)]
struct Input {
    cwd: Option<String>,
    stdin_json: Option<ValueRef>,
}
#[derive(NodeValue)]
struct Output {
    result: ValueRef,
}
struct ProcessNode {
    config: Config,
}

struct OwnedProcess {
    child: Child,
    closed: bool,
}
impl OwnedProcess {
    fn signal(&self, signal: i32) -> Result<(), Error> {
        if unsafe { libc::kill(-(self.child.id() as i32), signal) } == 0 {
            return Ok(());
        }
        let source = std::io::Error::last_os_error();
        if source.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        Err(source).context(IoSnafu {
            action: "signal owned process group",
        })
    }
    fn close(&mut self, graceful: bool) -> Result<(), Error> {
        if self.closed {
            return Ok(());
        }
        if graceful {
            self.signal(libc::SIGTERM)?;
            let until = Instant::now() + TERMINATION_GRACE;
            while self
                .child
                .try_wait()
                .context(IoSnafu {
                    action: "poll terminating process",
                })?
                .is_none()
                && Instant::now() < until
            {
                thread::sleep(Duration::from_millis(10));
            }
        }
        // A reaped leader does not imply its same-group descendants have stopped.
        self.signal(libc::SIGKILL)?;
        self.child.wait().context(IoSnafu {
            action: "reap owned process",
        })?;
        self.closed = true;
        Ok(())
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        let _ = self.close(false);
    }
}

impl ProcessNode {
    fn run(&self, input: Input, context: &ExecutionContext) -> Result<Output, Error> {
        if let Some(error) = context.cancellation().failure() {
            return Err(error.into());
        }
        let deadline = Instant::now() + Duration::from_millis(self.config.timeout_ms);
        let mut stdout = tempfile::tempfile().context(IoSnafu {
            action: "prepare process stdout",
        })?;
        let mut command = Command::new(&self.config.program);
        command
            .args(&self.config.args)
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .stdout(stdout.try_clone().context(IoSnafu {
                action: "attach process stdout",
            })?);
        if let Some(cwd) = &input.cwd {
            command.current_dir(cwd);
        }
        if let Some(value) = &input.stdin_json {
            let mut stdin = tempfile::tempfile().context(IoSnafu {
                action: "prepare JSON stdin",
            })?;
            serde_json::to_writer(&mut stdin, value).context(JsonSnafu)?;
            ensure!(
                stdin
                    .metadata()
                    .context(IoSnafu {
                        action: "measure JSON stdin"
                    })?
                    .len()
                    <= MAX_INPUT_BYTES,
                LimitSnafu { stream: "stdin" }
            );
            stdin.seek(SeekFrom::Start(0)).context(IoSnafu {
                action: "rewind JSON stdin",
            })?;
            command.stdin(stdin);
        }
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        let mut process = OwnedProcess {
            child: command.spawn().context(IoSnafu {
                action: "start process",
            })?,
            closed: false,
        };
        let outcome: Result<ExitStatus, Error> = (|| {
            loop {
                if let Some(error) = context.cancellation().failure() {
                    return Err(error.into());
                }
                ensure!(Instant::now() < deadline, TimeoutSnafu);
                ensure!(
                    stdout
                        .metadata()
                        .context(IoSnafu {
                            action: "measure process stdout"
                        })?
                        .len()
                        <= self.config.max_stdout_bytes,
                    LimitSnafu { stream: "stdout" }
                );
                if let Some(status) = process.child.try_wait().context(IoSnafu {
                    action: "poll process",
                })? {
                    return Ok(status);
                }
                thread::sleep(Duration::from_millis(10));
            }
        })();
        process.close(outcome.is_err())?;
        let status = outcome?;
        ensure!(status.success(), ExitSnafu { status });
        stdout.seek(SeekFrom::Start(0)).context(IoSnafu {
            action: "rewind process stdout",
        })?;
        let mut bytes = Vec::new();
        stdout
            .take(self.config.max_stdout_bytes + 1)
            .read_to_end(&mut bytes)
            .context(IoSnafu {
                action: "read process stdout",
            })?;
        ensure!(
            bytes.len() as u64 <= self.config.max_stdout_bytes,
            LimitSnafu { stream: "stdout" }
        );
        let result = match self.config.output {
            OutputFormat::Text => ValueRef::from(String::from_utf8(bytes).context(EncodingSnafu)?),
            OutputFormat::Json => ValueRef::from(
                serde_json::from_slice::<serde_json::Value>(&bytes).context(JsonSnafu)?,
            ),
        };
        Ok(Output {
            result: result.inherit_secrets(input.stdin_json.iter()),
        })
    }
}
impl TypedTaskNode for ProcessNode {
    type Input = Input;
    type Output = Output;
    fn execute(
        &self,
        input: Input,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Output>, NodeExecutionError> {
        Ok(self.run(input, context)?.into())
    }
}
fn factory(config: serde_json::Value) -> Result<PreparedNode, NodeBuildError> {
    let config: Config = deserialize_config(config)?;
    ensure!(
        !config.program.is_empty()
            && (1..=86_400_000).contains(&config.timeout_ms)
            && (1..=16 * 1024 * 1024).contains(&config.max_stdout_bytes),
        InvalidSnafu {
            message: "program, timeout_ms, and max_stdout_bytes must be valid"
        }
    );
    PreparedNode::typed_task(ProcessNode { config }, NodeMetadata::default())
}
inventory::submit! { NodeRegistration { kind: "process.run", factory: mf_runtime::NodeFactory::Plain(factory) } }
