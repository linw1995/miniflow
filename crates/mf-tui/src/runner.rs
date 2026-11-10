//! Shared executable invocation for native Flow values and conversation hosts.

use crate::{
    manifest::read_manifest,
    run::{LocalObservation, LocalOutcome, LocalRun, RunError},
};
use mf_runtime::{NodeValues, StreamCancellation, WorkflowArguments, WorkflowManifest};
use mf_telemetry::{description::ExecutionMode, identity::RunId};
use snafu::{ResultExt, Snafu, ensure};
use std::{path::Path, process::Command, time::Duration};

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
    #[snafu(display("runner manifest changed since preparation"))]
    ManifestChanged,
    #[snafu(display("runner output mode differs from its manifest"))]
    OutputMode,
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

/// The host supplies protocol policy; launch, transport, cancellation, draining,
/// and process-group cleanup are identical for all runner consumers.
pub struct RunnerInvocation<'a> {
    pub executable: &'a Path,
    pub arguments: &'a WorkflowArguments,
    /// Frozen preparation contract. The executable's manifest is reread and
    /// compared before every launch; observation is derived from that manifest.
    pub manifest: &'a WorkflowManifest,
    pub verbose: u8,
    pub output: RunnerOutput,
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
        let manifest = read_manifest(self.executable)?;
        ensure!(manifest == *self.manifest, ManifestChangedSnafu);
        let description = manifest.description()?;
        let streaming = description
            .execution
            .as_ref()
            .is_some_and(|execution| execution.mode == ExecutionMode::Stream);
        ensure!(
            streaming == matches!(self.output, RunnerOutput::Records),
            OutputModeSnafu
        );
        let arguments =
            mf_runtime::resolve_workflow_inputs(&manifest.input_schema, self.arguments, |name| {
                std::env::var_os(name)
            })?;
        manifest.input_schema.validate_stdin(&arguments, false)?;
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
        let mut command = Command::new(self.executable);
        command.arg("--inputs-file").arg(file.path());
        for _ in 0..self.verbose.min(3) {
            command.arg("--verbose");
        }
        command.env(
            mf_telemetry::SNAPSHOT_CAPTURE_ENV,
            if self.snapshots { "1" } else { "0" },
        );
        let run_id = RunId::new();
        let mut run = match self.output {
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
    fn manifest_and_input_preflight_fail_before_observation_or_process_effects() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("runner");
        let marker = root.path().join("executed");
        let expected = crate::test_support::sample_manifest();
        let script = format!("#!/bin/sh\ntouch '{}'\nprintf '{{}}'\n", marker.display());
        crate::test_support::write_runner(&path, &expected, &script);
        let invoke = |arguments: &WorkflowArguments, output| {
            RunnerInvocation {
                executable: &path,
                manifest: &expected,
                arguments,
                verbose: 0,
                output,
                snapshots: false,
            }
            .run(
                Default::default(),
                |_| panic!("preflight must not launch"),
                |_| Ok::<_, std::convert::Infallible>(()),
                || false,
            )
        };
        let finite = RunnerOutput::Finite { limit: 1024 };
        let arguments = WorkflowArguments::try_from(json!({"unknown": 1})).unwrap();
        assert!(matches!(
            invoke(&arguments, finite),
            Err(RunnerError::Inputs { .. })
        ));
        assert!(matches!(
            invoke(&Default::default(), RunnerOutput::Records),
            Err(RunnerError::OutputMode)
        ));
        let mut changed = expected.clone();
        changed.graph.definition = serde_json::from_value(json!({
            "version":"2026-10-03", "dependencies":{},
            "nodes":[{"id":"other", "kind":"builtin.constant", "config":{"value":1}}]
        }))
        .unwrap();
        changed.graph.execution_order = vec!["other".into()];
        changed
            .input_schema
            .inputs
            .insert("other".into(), Default::default());
        crate::test_support::write_runner(&path, &changed, &script);
        assert!(matches!(
            invoke(&Default::default(), finite),
            Err(RunnerError::ManifestChanged)
        ));
        std::fs::write(&path, &script).unwrap();
        assert!(matches!(
            invoke(&Default::default(), finite),
            Err(RunnerError::Manifest { .. })
        ));
        assert!(!marker.exists());
    }
}
