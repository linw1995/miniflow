//! Source-free, finite runner capabilities for ordinary Flow-valued edges.

use crate::{
    manifest::read_manifest,
    runner::{RunnerInvocation, RunnerOutput},
};
use mf_runtime::{ExecutionContext, FlowOutputs, FlowValue, PortSpec};
use snafu::{ResultExt, Snafu, ensure};
use std::{path::Path, sync::Arc};

#[derive(Debug, Snafu)]
pub enum RunnerFlowError {
    #[snafu(transparent)]
    Manifest {
        source: crate::manifest::ManifestReadError,
    },
    #[snafu(transparent)]
    Contract {
        source: mf_runtime::WorkflowManifestError,
    },
    #[snafu(transparent)]
    Build { source: mf_runtime::NodeBuildError },
    #[snafu(transparent)]
    Run { source: crate::runner::RunnerError },
    #[snafu(transparent)]
    Cancelled { source: mf_runtime::StreamError },
    #[snafu(display("runner Flow requires a finite callable without exported streams"))]
    NotFinite,
    #[snafu(display("runner output contract must match the manifest's names and optionality"))]
    Outputs,
    #[snafu(display("could not {operation}: {source}"))]
    Io {
        operation: &'static str,
        source: std::io::Error,
    },
    #[snafu(display("runner output is not valid JSON: {source}"))]
    Json { source: serde_json::Error },
    #[snafu(display("runner output capture failed: {source}"))]
    Capture {
        source: Arc<crate::run::CaptureFailure>,
    },
    #[snafu(display("runner exited with {status}: {stderr}"))]
    Exit {
        status: std::process::ExitStatus,
        stderr: String,
    },
}

/// Loads a built runner without its source or providers. Historical manifests
/// contain output names and optionality, but not provider output types: the host
/// supplies those types as an explicit contract, checked on every invocation.
/// Each invocation launches a fresh process and uses the existing LocalRun
/// supervisor, including process-group cancellation and bounded output capture.
pub fn load(path: &Path, outputs: Vec<PortSpec>) -> Result<FlowValue, RunnerFlowError> {
    load_with_observer(path, outputs, |_| {})
}

/// As [`load`], also publishes each invocation's independent live observation.
pub fn load_with_observer(
    path: &Path,
    outputs: Vec<PortSpec>,
    observe: impl Fn(crate::run::LocalObservation) + Send + Sync + 'static,
) -> Result<FlowValue, RunnerFlowError> {
    let executable = path.canonicalize().context(IoSnafu {
        operation: "resolve runner executable",
    })?;
    let manifest = read_manifest(&executable)?;
    let description = manifest.description()?;
    ensure!(
        manifest.graph.relations.is_callable()
            && manifest.graph.relations.streams.is_empty()
            && description.execution.as_ref().is_none_or(
                |execution| execution.mode == mf_telemetry::description::ExecutionMode::Single
            ),
        NotFiniteSnafu
    );
    ensure!(
        outputs.len() == manifest.graph.definition.outputs.len()
            && outputs.iter().all(|port| manifest
                .graph
                .definition
                .outputs
                .iter()
                .any(|output| output.name == port.name && output.optional != port.required)),
        OutputsSnafu
    );
    Ok(FlowValue::external(
        manifest.input_schema,
        outputs,
        move |context| {
            execute(&executable, description.clone(), context, &observe)
                .boxed()
                .context(mf_runtime::NodePluginFailedSnafu)
                .context(mf_runtime::NodeExecutionSnafu {
                    definition_id: "runner",
                })
        },
    )?)
}

fn execute(
    executable: &Path,
    description: mf_telemetry::description::WorkflowDescription,
    context: &mut ExecutionContext,
    observe: &dyn Fn(crate::run::LocalObservation),
) -> Result<FlowOutputs, RunnerFlowError> {
    if let Some(error) = context.cancellation().failure() {
        return Err(error.into());
    }
    let completion = RunnerInvocation {
        executable,
        arguments: context.workflow_arguments(),
        description,
        verbose: 0,
        output: RunnerOutput::Finite {
            limit: 256 * 1024 * 1024,
        },
        // Existing runner JSON cannot carry the caller's native privacy labels.
        snapshots: false,
    }
    .run(
        context.cancellation(),
        observe,
        |_| Ok::<_, std::convert::Infallible>(()),
        || false,
    )?;
    if let Some(error) = context.cancellation().failure() {
        return Err(error.into());
    }
    let outcome = completion.outcome;
    if let Some(source) = outcome.capture_failure {
        return Err(source).context(CaptureSnafu);
    }
    ensure!(
        outcome.status.success(),
        ExitSnafu {
            status: outcome.status,
            stderr: outcome.stderr
        }
    );
    let outputs: FlowOutputs =
        mf_runtime::decode_unique_json(&outcome.stdout).context(JsonSnafu)?;
    // JSON transport does not preserve native secret labels, including
    // secrets acquired inside the runner rather than from its arguments.
    Ok(outputs
        .into_iter()
        .map(|(name, value)| (name, value.into_secret()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

    fn script(root: &Path, contents: &str) -> std::path::PathBuf {
        let path = root.join("runner");
        fs::write(&path, format!("#!/bin/sh\n{contents}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn process_failures_and_duplicate_output_keys_preserve_typed_causes() {
        let root = tempfile::tempdir().unwrap();
        let description = crate::test_support::sample_manifest()
            .description()
            .unwrap();
        let path = script(root.path(), "echo failure >&2; exit 7");
        let error =
            execute(&path, description.clone(), &mut Default::default(), &|_| {}).unwrap_err();
        assert!(
            matches!(error, RunnerFlowError::Exit { status, stderr } if status.code() == Some(7) && stderr.contains("failure"))
        );
        let path = script(
            root.path(),
            "[ \"$MF_CAPTURE_SNAPSHOTS\" = 0 ] || exit 9\nprintf '%s' '{\"x\":1,\"x\":2}'",
        );
        let error = execute(&path, description, &mut Default::default(), &|_| {}).unwrap_err();
        assert!(matches!(error, RunnerFlowError::Json { .. }));
    }

    #[test]
    fn cancellation_reaps_a_running_process_and_prevents_a_cancelled_launch() {
        let root = tempfile::tempdir().unwrap();
        let path = script(root.path(), "exec sleep 60");
        let description = crate::test_support::sample_manifest()
            .description()
            .unwrap();
        let mut context = ExecutionContext::default();
        let cancel = context.cancellation();
        let started = std::time::Instant::now();
        let error = execute(&path, description.clone(), &mut context, &|_| {
            cancel.cancel(
                mf_runtime::StreamExecutionSnafu {
                    message: "test cancellation",
                }
                .build(),
            );
        })
        .unwrap_err();
        assert!(matches!(error, RunnerFlowError::Cancelled { .. }));
        assert!(started.elapsed() < Duration::from_secs(10));
        let error = execute(&path, description, &mut context, &|_| {
            panic!("cancelled launch")
        })
        .unwrap_err();
        assert!(matches!(error, RunnerFlowError::Cancelled { .. }));
    }
}
