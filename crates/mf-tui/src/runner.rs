//! Shared executable invocation for native Flow values and conversation hosts.

use crate::run::{LocalObservation, LocalOutcome, LocalRun, RunError};
use mf_runtime::{NodeValues, StreamCancellation, WorkflowArguments};
use mf_telemetry::{description::WorkflowDescription, identity::RunId};
use snafu::{ResultExt, Snafu, ensure};
use std::{io::Write, path::Path, process::Command, time::Duration};

#[derive(Debug, Snafu)]
pub enum RunnerError {
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
    pub description: WorkflowDescription,
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
        let mut file = tempfile::NamedTempFile::new().context(IoSnafu {
            operation: "create private runner arguments",
        })?;
        serde_json::to_writer(&mut file, self.arguments).context(ArgumentsSnafu)?;
        file.flush().context(IoSnafu {
            operation: "flush private runner arguments",
        })?;
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
            RunnerOutput::Finite { limit } => {
                LocalRun::start(command, self.description, run_id, limit)?
            }
            RunnerOutput::Records => {
                LocalRun::start_stream(command, self.description, run_id, cancellation.clone())?
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
