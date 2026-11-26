//! Shared executable invocation for native Flow values and conversation hosts.

use crate::{
    manifest::read_manifest,
    run::{LocalObservation, LocalOutcome, LocalRun, RunError},
};
use mf_runtime::{NodeValues, StreamCancellation, WorkflowArguments, WorkflowManifest};
use mf_telemetry::{description::WorkflowDescription, identity::RunId};
use snafu::{ResultExt, Snafu, ensure};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

#[derive(Debug, Snafu)]
pub enum RunnerError {
    #[snafu(transparent)]
    Manifest {
        source: crate::manifest::ManifestReadError,
    },
    #[snafu(transparent)]
    Contract {
        source: mf_runtime::WorkflowManifestError,
    },
    #[snafu(transparent)]
    Run { source: RunError },
    #[snafu(transparent)]
    Cancelled { source: mf_runtime::StreamError },
    #[snafu(transparent)]
    Inputs {
        source: mf_runtime::WorkflowInputError,
    },
    #[snafu(display("could not {operation}: {source}"))]
    Io {
        operation: &'static str,
        source: std::io::Error,
    },
    #[snafu(display("runner does not declare the requested v1 host callback"))]
    UnboundCallback,
    #[snafu(display("could not encode runner arguments: {source}"))]
    Arguments { source: serde_json::Error },
}

#[derive(Clone, Copy)]
pub enum RunnerOutput {
    /// One terminal JSON document, retained even after an unsuccessful exit.
    Finite { limit: u64 },
    /// Bounded, backpressured JSON records consumed while the process runs.
    Records,
}

/// An executable's immutable loading-time contract, reusable across invocations.
///
/// Loading reads and validates the embedded manifest once, then freezes observation
/// metadata and transport mode. The host must keep this executable unchanged for
/// the prepared runner's lifetime; replacement requires loading a new runner.
pub struct PreparedRunner {
    executable: PathBuf,
    manifest: WorkflowManifest,
    description: WorkflowDescription,
    output: RunnerOutput,
}

impl PreparedRunner {
    pub fn load(executable: &Path, terminal_limit: u64) -> Result<Self, RunnerError> {
        let executable = executable.canonicalize().context(IoSnafu {
            operation: "resolve runner executable",
        })?;
        let manifest = read_manifest(&executable)?;
        let description = manifest.description()?;
        let output = if description.is_streaming() {
            RunnerOutput::Records
        } else {
            RunnerOutput::Finite {
                limit: terminal_limit,
            }
        };
        Ok(Self {
            executable,
            manifest,
            description,
            output,
        })
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn manifest(&self) -> &WorkflowManifest {
        &self.manifest
    }

    pub fn description(&self) -> &WorkflowDescription {
        &self.description
    }

    pub fn is_streaming(&self) -> bool {
        self.description.is_streaming()
    }
}

/// Only per-invocation inputs and process policy vary; executable metadata and
/// transport mode come from a previously loaded runner.
pub struct RunnerInvocation<'a> {
    pub runner: &'a PreparedRunner,
    pub arguments: &'a WorkflowArguments,
    pub verbose: u8,
    pub snapshots: bool,
}

/// Preserve raw completion on protocol failure so hosts can reconcile or recover
/// a terminal reply. A consumer failure stops callbacks, cancels the process, and
/// drains pending records before returning the original typed error.
pub struct RunnerCompletion<E> {
    pub outcome: LocalOutcome,
    pub protocol_failure: Option<E>,
}

impl RunnerInvocation<'_> {
    pub fn run<E>(
        self,
        cancellation: StreamCancellation,
        observe: impl FnOnce(LocalObservation),
        mut consume: impl FnMut(NodeValues) -> Result<(), E>,
        should_cancel: impl FnMut() -> bool,
    ) -> Result<RunnerCompletion<E>, RunnerError> {
        self.run_batches(
            cancellation,
            observe,
            |records| {
                for record in records {
                    consume(record)?;
                }
                Ok(())
            },
            should_cancel,
        )
    }

    /// Consume available records in wire order without waiting for another record to fill a batch.
    pub fn run_batches<E>(
        self,
        cancellation: StreamCancellation,
        observe: impl FnOnce(LocalObservation),
        consume: impl FnMut(&mut dyn Iterator<Item = NodeValues>) -> Result<(), E>,
        should_cancel: impl FnMut() -> bool,
    ) -> Result<RunnerCompletion<E>, RunnerError> {
        self.run_with_callbacks(
            Default::default(),
            cancellation,
            observe,
            consume,
            should_cancel,
        )
    }

    pub fn run_with_callbacks<E>(
        self,
        callbacks: mf_runtime::callback::HostCallbacks,
        cancellation: StreamCancellation,
        observe: impl FnOnce(LocalObservation),
        mut consume: impl FnMut(&mut dyn Iterator<Item = NodeValues>) -> Result<(), E>,
        mut should_cancel: impl FnMut() -> bool,
    ) -> Result<RunnerCompletion<E>, RunnerError> {
        if should_cancel() {
            cancel(&cancellation);
        }
        if let Some(error) = cancellation.failure() {
            return Err(error.into());
        }
        let schema = &self.runner.manifest.descriptor.input_schema;
        let arguments = mf_runtime::resolve_workflow_inputs(schema, self.arguments, |name| {
            std::env::var_os(name)
        })?;
        schema.validate_stdin(&arguments, false)?;
        let mut file = tempfile::NamedTempFile::new().context(IoSnafu {
            operation: "create private runner arguments",
        })?;
        serde_json::to_writer(&mut file, &arguments).context(ArgumentsSnafu)?;
        let size = file
            .as_file()
            .metadata()
            .context(IoSnafu {
                operation: "inspect private runner arguments",
            })?
            .len();
        ensure!(
            size <= mf_runtime::MAX_WORKFLOW_INPUT_BYTES as u64,
            mf_runtime::WorkflowInputTooLargeSnafu {
                limit: mf_runtime::MAX_WORKFLOW_INPUT_BYTES
            }
        );
        if should_cancel() {
            cancel(&cancellation);
        }
        if let Some(error) = cancellation.failure() {
            return Err(error.into());
        }
        let mut command = Command::new(&self.runner.executable);
        command.arg("--inputs-file").arg(file.path());
        for _ in 0..self.verbose.min(3) {
            command.arg("--verbose");
        }
        command.env(
            mf_telemetry::SNAPSHOT_CAPTURE_ENV,
            if self.snapshots { "1" } else { "0" },
        );
        let run_id = RunId::new();
        let description = self.runner.description.clone();
        for name in callbacks.keys() {
            ensure!(
                self.runner.manifest.descriptor.callbacks.contains_key(name),
                UnboundCallbackSnafu
            );
        }
        let mut run = LocalRun::start_with_callbacks(
            command,
            description,
            run_id,
            cancellation.clone(),
            callbacks,
            self.runner.output,
        )?;
        observe(run.observation());
        let mut protocol_failure = None;
        loop {
            if should_cancel() {
                cancel(&cancellation);
            }
            if cancellation.failure().is_some() {
                run.cancel();
            }
            consume_records(&mut run, &mut consume, &mut protocol_failure);
            if let Some(outcome) = run.poll()? {
                consume_records(&mut run, &mut consume, &mut protocol_failure);
                return Ok(RunnerCompletion {
                    outcome,
                    protocol_failure,
                });
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn cancel(cancellation: &StreamCancellation) {
    cancellation.cancel(
        mf_runtime::StreamExecutionSnafu {
            message: "runner invocation cancelled",
        }
        .build(),
    );
}

fn consume_records<E>(
    run: &mut LocalRun,
    consume: &mut impl FnMut(&mut dyn Iterator<Item = NodeValues>) -> Result<(), E>,
    failure: &mut Option<E>,
) {
    if failure.is_none() {
        let mut records = std::iter::from_fn(|| run.try_record().ok());
        if let Err(error) = consume(&mut records) {
            *failure = Some(error);
            run.cancel();
        }
    }
    if failure.is_some() {
        // Drain after a protocol failure so the child can close stdout.
        while run.try_record().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn loading_freezes_metadata_and_invocations_only_bind_current_inputs() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("runner");
        let marker = root.path().join("executed");
        let manifest = crate::test_support::sample_manifest();
        let script = format!(
            "#!/bin/sh\n[ \"$#\" -eq 2 ] && [ \"$1\" = --inputs-file ] || exit 19\n[ -z \"${{MF_RUNNER_RPC_V1+x}}\" ] || exit 20\nprintf x >> '{}'\nprintf '%s\\n' '{{}}'\n",
            marker.display()
        );
        crate::test_support::write_runner(&path, &manifest, &script);
        let runner = PreparedRunner::load(&path, 1024).unwrap();
        assert_eq!(runner.manifest(), &manifest);
        assert_eq!(runner.description(), &manifest.description().unwrap());
        assert!(!runner.is_streaming());
        let invoke = |arguments: &WorkflowArguments| {
            RunnerInvocation {
                runner: &runner,
                arguments,
                verbose: 0,
                snapshots: false,
            }
            .run(
                Default::default(),
                |_| {},
                |_| Ok::<_, std::convert::Infallible>(()),
                || false,
            )
        };
        let arguments = WorkflowArguments::try_from(json!({"unknown": 1})).unwrap();
        assert!(matches!(
            invoke(&arguments),
            Err(RunnerError::Inputs { .. })
        ));
        assert!(!marker.exists());
        for _ in 0..2 {
            assert!(
                invoke(&Default::default())
                    .unwrap()
                    .outcome
                    .status
                    .success()
            );
        }
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "xx");
        // A missing artifact fails at process startup, not by attempting to reread
        // a previously prepared contract. Metadata remains available to the host.
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            invoke(&Default::default()),
            Err(RunnerError::Run { .. })
        ));
        assert_eq!(runner.manifest(), &manifest);
    }

    #[test]
    fn only_the_launched_pid_can_invoke_callbacks_and_stdout_is_always_business_data() {
        use mf_runtime::callback::{CallbackReply, HostCallback};
        use std::sync::{Arc, Mutex};
        use std::time::Instant;
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let events = delivered.clone();
        let callbacks = [(
            "test".into(),
            HostCallback::new(move |args, _| {
                let events = events.clone();
                let size = args.0.get("size").and_then(|n| n.as_u64()).unwrap_or(1) as usize;
                Ok(
                    CallbackReply::from(NodeValues::from([("x".into(), "x".repeat(size).into())]))
                        .on_delivery(move |ok| events.lock().unwrap().push(ok)),
                )
            }),
        )]
        .into();
        let run = |script: &str, output| {
            let mut command = Command::new("python3");
            command.args([
                "-c",
                &format!(
                    r#"
import json, os, socket, stat, subprocess, sys
launch = json.loads(os.environ['MF_RUNNER_RPC_V1'])
assert launch['bindings'] == ['test'] and launch['parent'] == os.getppid()
path = launch['socket']
assert stat.S_IMODE(os.stat(os.path.dirname(path)).st_mode) == 0o700
assert sys.stdin.read() == ''
def connect():
    channel = socket.socket(socket.AF_UNIX)
    channel.connect(path)
    return channel.makefile('rwb', buffering=0)
def call(channel, n, name='test'):
    request = dict(id=n, binding=name, args=dict(values={{}}))
    channel.write(json.dumps(request).encode() + b'\n')
    return channel.readline()
{script}"#
                ),
            ]);
            let mut run = LocalRun::start_with_callbacks(
                command,
                crate::test_support::runner_description(matches!(output, RunnerOutput::Records)),
                RunId::new(),
                Default::default(),
                mf_runtime::callback::HostCallbacks::clone(&callbacks),
                output,
            )
            .unwrap();
            let start = Instant::now();
            let mut records = Vec::new();
            loop {
                while let Ok(record) = run.try_record() {
                    records.push(record);
                }
                if let Some(outcome) = run.poll().unwrap() {
                    while let Ok(record) = run.try_record() {
                        records.push(record);
                    }
                    break (outcome, records);
                }
                assert!(start.elapsed() < Duration::from_secs(10));
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        let request = json!({"id":1,"binding":"test","args":{"values":{}}});
        let script = format!(
            r#"
foreign = "import socket,sys;s=socket.socket(socket.AF_UNIX);s.connect(sys.argv[1]);s.sendall(sys.argv[2].encode());assert not s.recv(1)"
subprocess.run([sys.executable,'-c',foreign,path,{line:?}], stderr=subprocess.DEVNULL)
channel = connect()
print({line:?}, end='', flush=True)
for n in [1,2]:
    assert json.loads(call(channel,n)) == dict(id=n,value=dict(values=dict(x='x')))
print('{{"type":"result"}}')"#,
            line = format!("{request}\n")
        );
        let (outcome, records) = run(&script, RunnerOutput::Records);
        assert!(
            outcome.status.success() && outcome.capture_error.is_none(),
            "{} {:?}",
            outcome.stderr,
            outcome.capture_error
        );
        assert_eq!(
            serde_json::to_value(&records).unwrap(),
            json!([request,{"type":"result"}])
        );
        assert!(outcome.stdout.is_empty());
        for (script, diagnostic) in [
            ("print('bad')", "invalid workflow output record"),
            ("call(connect(),1,'other')", "unbound callback"),
            ("call(connect(),2)", "out-of-order"),
        ] {
            let (outcome, _) = run(script, RunnerOutput::Records);
            assert!(outcome.capture_error.unwrap().contains(diagnostic));
        }
        assert_eq!(*delivered.lock().unwrap(), [true, true]);
    }

    #[test]
    fn invalid_artifacts_fail_during_loading() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("runner");
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        assert!(matches!(
            PreparedRunner::load(&path, 1024),
            Err(RunnerError::Manifest { .. })
        ));
    }
}
