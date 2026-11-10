//! Source-free runner capabilities for ordinary Flow-valued edges.

use crate::{
    manifest::read_manifest,
    runner::{RunnerError, RunnerInvocation, RunnerOutput},
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
    Build { source: mf_runtime::NodeBuildError },
    #[snafu(transparent)]
    Run { source: crate::runner::RunnerError },
    #[snafu(transparent)]
    Delivery { source: mf_runtime::StreamError },
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
    let manifest = Arc::new(read_manifest(&executable)?);
    let expected = Arc::clone(&manifest);
    Ok(FlowValue::external(
        &manifest,
        outputs,
        move |context, sink| {
            execute(&executable, &expected, context, sink, &observe)
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
    manifest: &mf_runtime::WorkflowManifest,
    context: &mut ExecutionContext,
    sink: &mut dyn mf_runtime::EmissionSink,
    observe: &dyn Fn(crate::run::LocalObservation),
) -> Result<FlowOutputs, RunnerFlowError> {
    let streaming = manifest.graph.definition.execution.is_some();
    let completion = RunnerInvocation {
        executable,
        arguments: context.workflow_arguments(),
        manifest,
        verbose: 0,
        output: if streaming {
            RunnerOutput::Records
        } else {
            RunnerOutput::Finite {
                limit: 256 * 1024 * 1024,
            }
        },
        // Existing runner JSON cannot carry the caller's native privacy labels.
        snapshots: false,
    }
    .run(
        context.cancellation(),
        observe,
        |outputs| {
            sink.send(mf_runtime::NodeResult {
                outputs: secret_outputs(outputs),
                ..Default::default()
            })
        },
        || false,
    )?;
    if let Some(error) = completion.protocol_failure {
        return Err(error.into());
    }
    if let Some(error) = context.cancellation().failure() {
        return Err(RunnerError::from(error).into());
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
    if streaming {
        return Ok(FlowOutputs::new());
    }
    Ok(secret_outputs(
        mf_runtime::decode_unique_json(&outcome.stdout).context(JsonSnafu)?,
    ))
}

fn secret_outputs(outputs: FlowOutputs) -> FlowOutputs {
    // JSON transport does not preserve native secret labels, including
    // secrets acquired inside the runner rather than from its arguments.
    outputs
        .into_iter()
        .map(|(name, value)| (name, value.into_secret()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn script(root: &Path, contents: &str) -> std::path::PathBuf {
        let path = root.join("runner");
        crate::test_support::write_runner(
            &path,
            &crate::test_support::sample_manifest(),
            &format!("#!/bin/sh\n{contents}\n"),
        );
        path
    }

    #[test]
    fn process_failures_and_duplicate_output_keys_preserve_typed_causes() {
        let root = tempfile::tempdir().unwrap();
        let manifest = crate::test_support::sample_manifest();
        let path = script(root.path(), "echo failure >&2; exit 7");
        let error = execute(
            &path,
            &manifest,
            &mut Default::default(),
            &mut mf_runtime::DiscardSink,
            &|_| {},
        )
        .unwrap_err();
        assert!(
            matches!(error, RunnerFlowError::Exit { status, stderr } if status.code() == Some(7) && stderr.contains("failure"))
        );
        let path = script(
            root.path(),
            "[ \"$MF_CAPTURE_SNAPSHOTS\" = 0 ] || exit 9\nprintf '%s' '{\"x\":1,\"x\":2}'",
        );
        let error = execute(
            &path,
            &manifest,
            &mut Default::default(),
            &mut mf_runtime::DiscardSink,
            &|_| {},
        )
        .unwrap_err();
        assert!(matches!(error, RunnerFlowError::Json { .. }));
    }

    #[test]
    fn cancellation_reaps_a_running_process_and_prevents_a_cancelled_launch() {
        let root = tempfile::tempdir().unwrap();
        let path = script(root.path(), "exec sleep 60");
        let manifest = crate::test_support::sample_manifest();
        let mut context = ExecutionContext::default();
        let cancel = context.cancellation();
        let started = std::time::Instant::now();
        let error = execute(
            &path,
            &manifest,
            &mut context,
            &mut mf_runtime::DiscardSink,
            &|_| {
                cancel.cancel(
                    mf_runtime::StreamExecutionSnafu {
                        message: "test cancellation",
                    }
                    .build(),
                );
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            RunnerFlowError::Run {
                source: RunnerError::Cancelled { .. }
            }
        ));
        assert!(started.elapsed() < Duration::from_secs(10));
        let error = execute(
            &path,
            &manifest,
            &mut context,
            &mut mf_runtime::DiscardSink,
            &|_| panic!("cancelled launch"),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            RunnerFlowError::Run {
                source: RunnerError::Cancelled { .. }
            }
        ));
    }

    #[test]
    fn record_runner_delivers_live_secret_values_and_reaps_on_sink_failure() {
        use mf_runtime::{EmissionSink, NodeResult, ValueType};
        struct Sink {
            gate: std::path::PathBuf,
            values: Vec<i64>,
            reject: bool,
        }
        impl EmissionSink for Sink {
            fn send(&mut self, result: NodeResult) -> Result<(), mf_runtime::StreamError> {
                if self.reject {
                    return mf_runtime::StreamExecutionSnafu {
                        message: "receiver rejected record",
                    }
                    .fail();
                }
                let value = &result.outputs["value"];
                assert!(value.is_secret());
                self.values.push(value.as_i64().unwrap());
                std::fs::write(&self.gate, "release").unwrap();
                Ok(())
            }
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("runner");
        let gate = root.path().join("release");
        let mut manifest = crate::test_support::sample_manifest();
        manifest.graph.definition = serde_json::from_value(serde_json::json!({
            "version":"2026-10-28", "dependencies":{}, "execution":{},
            "nodes":[{"id":"source", "kind":"builtin.constant", "config":{"value":1}}],
            "outputs":[{"name":"value", "node":"source", "port":"value"}]
        }))
        .unwrap();
        manifest.graph.execution_order = vec!["source".into()];
        manifest
            .input_schema
            .inputs
            .insert("source".into(), Default::default());
        crate::test_support::write_runner(
            &path,
            &manifest,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' '{{\"value\":1}}'\ni=0\nwhile [ ! -f '{}' ] && [ \"$i\" -lt 500 ]; do sleep 0.01; i=$((i+1)); done\n[ -f '{}' ] || exit 9\nprintf '%s\\n' '{{\"value\":2}}'\n",
                gate.display(),
                gate.display()
            ),
        );
        let flow = load(&path, vec![PortSpec::new("value", ValueType::Int64, true)]).unwrap();
        assert!(flow.output_ports().is_empty());
        assert_eq!(
            flow.stream_output_ports(),
            [PortSpec::new("value", ValueType::Int64, false)]
        );
        let mut sink = Sink {
            gate,
            values: vec![],
            reject: false,
        };
        assert!(
            flow.invoke_with_sink(
                "runner",
                Default::default(),
                &mut Default::default(),
                &mut sink
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(sink.values, [1, 2]);
        std::fs::remove_file(&sink.gate).unwrap();
        sink.reject = true;
        let mut context = ExecutionContext::default();
        let started = std::time::Instant::now();
        let error = flow
            .invoke_with_sink("runner", Default::default(), &mut context, &mut sink)
            .unwrap_err();
        assert!(error.to_string().contains("receiver rejected record"));
        assert!(context.cancellation().failure().is_some());
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
