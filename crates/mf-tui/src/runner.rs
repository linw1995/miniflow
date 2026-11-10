//! Shared executable invocation for native Flow values and conversation hosts.

use crate::{
    manifest::read_manifest,
    run::{LocalObservation, LocalOutcome, LocalRun, RunError},
};
use mf_runtime::{NodeValues, StreamCancellation, WorkflowArguments, WorkflowManifest};
use mf_telemetry::{
    description::{ExecutionMode, WorkflowDescription},
    identity::RunId,
};
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
        let output = if description
            .execution
            .as_ref()
            .is_some_and(|execution| execution.mode == ExecutionMode::Stream)
        {
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
        matches!(self.output, RunnerOutput::Records)
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
        mut should_cancel: impl FnMut() -> bool,
    ) -> Result<RunnerCompletion<E>, RunnerError> {
        if should_cancel() {
            cancel(&cancellation);
        }
        if let Some(error) = cancellation.failure() {
            return Err(error.into());
        }
        let schema = &self.runner.manifest.input_schema;
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
        let mut run = match self.runner.output {
            RunnerOutput::Finite { limit } => LocalRun::start(command, description, run_id, limit)?,
            RunnerOutput::Records => {
                LocalRun::start_stream(command, description, run_id, cancellation.clone())?
            }
        };
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
    consume: &mut impl FnMut(NodeValues) -> Result<(), E>,
    failure: &mut Option<E>,
) {
    while let Ok(record) = run.try_record() {
        if failure.is_none()
            && let Err(error) = consume(record)
        {
            *failure = Some(error);
            run.cancel();
        }
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
            "#!/bin/sh\nprintf x >> '{}'\nprintf '{{}}'\n",
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
