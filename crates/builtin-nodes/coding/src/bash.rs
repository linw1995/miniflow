use crate::{check_cancelled, error::*, private_text, workspace::WorkspaceRoot};
use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeFactory, NodeRegistration, NodeValue,
    Nullable, PreparedNode, Shared, TypedNodeResult, TypedTaskNode, deserialize_config,
};
use rustix::{
    fs::{Access, OFlags, access, fcntl_getfl, fcntl_setfl},
    io::Errno,
    process::{Pid, Signal, WaitId, WaitIdOptions, kill_process, kill_process_group, waitid},
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{OptionExt, ResultExt, ensure};
use std::{
    env,
    ffi::OsStr,
    io::{self, Read},
    os::{
        fd::AsFd,
        unix::process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

pub const BASH_KIND: &str = "builtin.coding.bash";

const POLL_INTERVAL: Duration = Duration::from_millis(5);
const CHUNK_BYTES: usize = 8192;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Shell {
    path: Option<String>,
    #[serde(default)]
    command_prefix: String,
}

impl Shell {
    fn validate(&self) -> Result<(), CodingError> {
        if let Some(path) = &self.path {
            ensure!(
                !path.contains('\0') && (Path::new(path).is_absolute() || path.starts_with("~/")),
                InvalidConfigSnafu {
                    field: "shell.path",
                    requirement: "an absolute or ~/ shell executable path"
                }
            );
        }
        ensure!(
            !self.command_prefix.contains('\0'),
            InvalidConfigSnafu {
                field: "shell.command_prefix",
                requirement: "no NUL bytes"
            }
        );
        Ok(())
    }

    fn executable(&self, search_path: Option<&OsStr>) -> Result<PathBuf, CodingError> {
        if let Some(path) = &self.path {
            let path = Path::new(path);
            if let Ok(relative) = path.strip_prefix("~") {
                return Ok(env::home_dir()
                    .filter(|home| home.is_absolute())
                    .context(InvalidConfigSnafu {
                        field: "shell.path",
                        requirement: "a home directory to resolve ~/",
                    })?
                    .join(relative));
            }
            return Ok(path.to_owned());
        }
        let candidates = search_path
            .into_iter()
            .flat_map(env::split_paths)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join("bash"));
        Ok(std::iter::once(PathBuf::from("/bin/bash"))
            .chain(candidates)
            .find(|path| path.is_file() && access(path, Access::EXEC_OK).is_ok())
            .unwrap_or_else(|| PathBuf::from("/bin/sh")))
    }

    fn command(&self, source: String) -> Result<Command, CodingError> {
        ensure!(
            !source.contains('\0'),
            InvalidInputSnafu {
                field: "command",
                requirement: "shell text without NUL bytes"
            }
        );
        let search_path = env::var_os("PATH");
        let mut command = Command::new(self.executable(search_path.as_deref())?);
        let source = if self.command_prefix.is_empty() {
            source
        } else {
            format!("{}\n{source}", self.command_prefix)
        };
        command.arg("-c").arg(source).env_clear();
        // Keep the toolchain available without exporting the runner's provider credentials.
        if let Some(path) = search_path {
            command.env("PATH", path);
        }
        Ok(command)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    workspace_root: Value,
    #[serde(default)]
    shell: Shell,
    timeout_ms: Option<u64>,
    #[serde(default = "default_output_bytes")]
    max_output_bytes: usize,
}

const fn default_output_bytes() -> usize {
    50 * 1024
}

struct BashNode {
    root: WorkspaceRoot,
    shell: Shell,
    timeout: Option<Duration>,
    max_output_bytes: usize,
}

#[derive(NodeValue)]
struct BashInputs {
    command: String,
    timeout: Option<Nullable<f64>>,
}

#[derive(NodeValue)]
struct BashOutputs {
    exit_code: Option<i64>,
    signal: Option<i64>,
    timed_out: bool,
    stdout: Shared<String>,
    stderr: Shared<String>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

impl TypedTaskNode for BashNode {
    type Input = BashInputs;
    type Output = BashOutputs;

    fn execute(
        &self,
        input: BashInputs,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<BashOutputs>, NodeExecutionError> {
        check_cancelled(context)?;
        let timeout = match input.timeout {
            Some(Nullable::Value(seconds)) => {
                let requested = Duration::try_from_secs_f64(seconds).context(ShellTimeoutSnafu)?;
                ensure!(
                    !requested.is_zero(),
                    InvalidInputSnafu {
                        field: "timeout",
                        requirement: "a positive duration in seconds"
                    }
                );
                Some(self.timeout.map_or(requested, |cap| cap.min(requested)))
            }
            _ => self.timeout,
        };
        let workspace = self.root.resolve(context)?;
        let mut command = self.shell.command(input.command)?;
        command
            .current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let started = Instant::now();
        let mut process = OwnedProcess::start(&mut command)?;
        let mut stdout = Capture::new(
            process.child.stdout.take().expect("piped stdout"),
            self.max_output_bytes,
        )?;
        let mut stderr = Capture::new(
            process.child.stderr.take().expect("piped stderr"),
            self.max_output_bytes,
        )?;
        let timed_out = loop {
            check_cancelled(context)?;
            if process.exited()? {
                break false;
            }
            let remaining = timeout.map(|limit| limit.saturating_sub(started.elapsed()));
            if remaining == Some(Duration::ZERO) {
                break true;
            }
            stdout.drain(8)?;
            stderr.drain(8)?;
            thread::sleep(remaining.map_or(POLL_INTERVAL, |left| POLL_INTERVAL.min(left)));
        };
        let status = process.close()?;
        let (stdout, stdout_truncated) = stdout.finish()?;
        let (stderr, stderr_truncated) = stderr.finish()?;
        check_cancelled(context)?;
        let mut result = TypedNodeResult::from(BashOutputs {
            exit_code: if timed_out {
                None
            } else {
                status.code().map(i64::from)
            },
            signal: if timed_out {
                None
            } else {
                status.signal().map(i64::from)
            },
            timed_out,
            stdout: private_text(stdout),
            stderr: private_text(stderr),
            stdout_truncated,
            stderr_truncated,
        });
        if result.outputs.exit_code.is_none() {
            result.skipped.insert("exit_code".into());
        }
        if result.outputs.signal.is_none() {
            result.skipped.insert("signal".into());
        }
        Ok(result)
    }
}

struct OwnedProcess {
    child: Child,
    group: Pid,
    closed: bool,
}

impl OwnedProcess {
    fn start(command: &mut Command) -> Result<Self, CodingError> {
        let child = command.spawn().context(IoSnafu {
            operation: "start shell command",
        })?;
        let group = Pid::from_child(&child);
        Ok(Self {
            child,
            group,
            closed: false,
        })
    }

    fn exited(&self) -> Result<bool, CodingError> {
        // Retain the leader PID until group cleanup, even when the command exited normally.
        loop {
            let status = waitid(
                WaitId::Pid(self.group),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            );
            if !matches!(status, Err(Errno::INTR)) {
                return Ok(status
                    .map_err(io::Error::from)
                    .context(IoSnafu {
                        operation: "poll shell command",
                    })?
                    .is_some());
            }
        }
    }

    fn close(&mut self) -> Result<ExitStatus, CodingError> {
        let group_result = kill_process_group(self.group, Signal::KILL);
        let exited = self.exited()?;
        if !exited {
            // The direct child might have left its original group; it is still ours to reap.
            match kill_process(self.group, Signal::KILL) {
                Ok(()) | Err(Errno::SRCH) => (),
                Err(error) => {
                    return Err(io::Error::from(error)).context(IoSnafu {
                        operation: "terminate shell command",
                    });
                }
            }
        }
        let status = self.child.wait().context(IoSnafu {
            operation: "reap shell command",
        })?;
        self.closed = true;
        match group_result {
            Ok(()) | Err(Errno::SRCH) => Ok(status),
            // Darwin reports EPERM for a group containing only zombie processes.
            Err(Errno::PERM) if cfg!(target_os = "macos") && exited => Ok(status),
            Err(error) => Err(io::Error::from(error)).context(IoSnafu {
                operation: "terminate command process group",
            }),
        }
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.close();
        }
    }
}

struct Capture<R> {
    pipe: R,
    prefix: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl<R: AsFd + Read> Capture<R> {
    fn new(pipe: R, limit: usize) -> Result<Self, CodingError> {
        fcntl_getfl(&pipe)
            .and_then(|flags| fcntl_setfl(&pipe, flags | OFlags::NONBLOCK))
            .map_err(io::Error::from)
            .context(IoSnafu {
                operation: "configure command output pipe",
            })?;
        Ok(Self {
            pipe,
            prefix: Vec::new(),
            limit,
            truncated: false,
        })
    }

    fn drain(&mut self, max_reads: usize) -> Result<bool, CodingError> {
        let mut buffer = [0; CHUNK_BYTES];
        for _ in 0..max_reads {
            match self.pipe.read(&mut buffer) {
                Ok(0) => return Ok(false),
                Ok(bytes) => {
                    let retained = bytes.min(self.limit - self.prefix.len());
                    self.prefix.extend_from_slice(&buffer[..retained]);
                    self.truncated |= retained < bytes;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    return Err(error).context(IoSnafu {
                        operation: "read command output",
                    });
                }
            }
        }
        Ok(true)
    }

    fn finish(mut self) -> Result<(String, bool), CodingError> {
        // A descendant outside the group may still own the other end. Never wait for EOF.
        self.truncated |= self.drain(2 * MAX_OUTPUT_BYTES / CHUNK_BYTES)?;
        Ok((
            String::from_utf8_lossy(&self.prefix).into_owned(),
            self.truncated,
        ))
    }
}

fn prepare(value: Value) -> Result<PreparedNode, NodeBuildError> {
    let config: Config = deserialize_config(value)?;
    config.shell.validate()?;
    ensure!(
        config.timeout_ms != Some(0),
        InvalidConfigSnafu {
            field: "timeout_ms",
            requirement: "a positive time limit in milliseconds"
        }
    );
    ensure!(
        (1..=MAX_OUTPUT_BYTES).contains(&config.max_output_bytes),
        InvalidConfigSnafu {
            field: "max_output_bytes",
            requirement: "1 to 1048576 bytes per output stream"
        }
    );
    let root = WorkspaceRoot::prepare(config.workspace_root)?;
    let metadata = root.metadata();
    PreparedNode::typed_task(
        BashNode {
            root,
            shell: config.shell,
            timeout: config.timeout_ms.map(Duration::from_millis),
            max_output_bytes: config.max_output_bytes,
        },
        metadata,
    )
}

inventory::submit! {
    NodeRegistration { kind: BASH_KIND, factory: NodeFactory::Plain(prepare) }
}
