#[path = "fixtures/observation_capture.rs"]
mod capture;
mod common;
#[path = "fixtures/multi-nodes/src/context_fixture.rs"]
mod fixture;

use capture::Harness;
use mf_compiler::{CompiledWorkflow, NodeRegistry, execute_compiled, plan_definition};
use mf_telemetry::{
    event::{Event, Outcome},
    identity::RunId,
    wire::WireRecord,
};
use serde_json::{Value, json};

fn plan(config: Value) -> CompiledWorkflow {
    plan_definition(&serde_json::from_value(json!({
        "version":"2026-09-26", "dependencies":{},
        "nodes":[
            {"id":"a", "kind":"fixture.context", "config":config},
            {"id":"b", "kind":"fixture.context", "config":{"inputs":["input"],"ports":["value"],"outputs":{"value":2}}},
            {"id":"c", "kind":"fixture.context", "config":{"ports":[],"fail":true}}
        ],
        "edges":[{"from_node":"a","from_output":"value","to_node":"b","to_input":"input"}],
        "control_edges":[{"from_node":"a","from_output":"inactive","to_node":"c"}],
        "outputs":[{"name":"answer","node":"b","port":"value","optional":true}]
    })).unwrap()).unwrap()
}

fn success_config() -> Value {
    json!({"ports":["value","inactive"],"outputs":{"value":1},"skipped":["inactive"]})
}

fn run(
    plan: &CompiledWorkflow,
    sample: bool,
) -> (
    Result<mf_runtime::FlowOutputs, mf_compiler::WorkflowExecutionError>,
    Harness,
) {
    let harness = Harness::new(sample);
    let observation = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    let result = execute_compiled(
        plan,
        &NodeRegistry::from_inventory().unwrap(),
        Some(observation),
    );
    (result, harness)
}

fn finish(records: &[WireRecord]) -> Event {
    records.last().unwrap().decode().unwrap().event
}

#[test]
fn execution_matches_unobserved_results_with_skips_and_native_span_parentage() {
    let plan = plan(success_config());
    let expected = execute_compiled(&plan, &NodeRegistry::from_inventory().unwrap(), None).unwrap();
    let (actual, harness) = run(&plan, true);
    assert_eq!(actual.unwrap(), expected);
    let records = harness.records();
    assert_eq!(records.len(), 7);
    for (index, record) in records.iter().enumerate() {
        assert_eq!(record.decode().unwrap().sequence.get(), index as i64 + 1);
    }
    assert!(
        matches!(finish(&records), Event::WorkflowFinished { outcome: Outcome::Succeeded, visited_node_count, final_sequence, .. } if visited_node_count.get() == 3 && final_sequence.get() == 7)
    );
    let skipped = records
        .iter()
        .find(|r| r.event_name == "mf.node.skipped")
        .unwrap();
    assert_eq!(skipped.attributes["mf.node.id"], "c");
    assert_eq!(
        skipped.body["causes"],
        json!([{"source_node":"a","source_output":"inactive"}])
    );
    let spans = harness.spans.get_finished_spans().unwrap();
    let root = spans
        .iter()
        .find(|span| span.name == "mf.workflow")
        .unwrap();
    assert_eq!(spans.len(), 4);
    for span in spans.iter().filter(|span| span.name == "mf.node") {
        assert_eq!(span.parent_span_id, root.span_context.span_id());
        assert_eq!(span.span_context.trace_id(), root.span_context.trace_id());
        assert!(
            records
                .iter()
                .any(|r| r.trace_context.as_ref().unwrap().span_id
                    == span.span_context.span_id().to_string())
        );
        assert!(!matches!(
            span.status,
            opentelemetry::trace::Status::Error { .. }
        ));
    }
}

#[test]
fn sampling_and_disabled_diagnostic_checks_do_not_suppress_lifecycle_logs() {
    let (result, harness) = run(&plan(success_config()), false);
    assert!(result.is_ok());
    assert_eq!(harness.records().len(), 7);
    assert!(harness.spans.get_finished_spans().unwrap().is_empty());
    assert!(
        harness
            .records()
            .iter()
            .all(|record| record.trace_context.as_ref().unwrap().trace_flags == 0)
    );
}

#[test]
fn noop_tracers_leave_logs_without_invalid_native_correlation() {
    use opentelemetry::{
        Context,
        trace::{TraceContextExt, Tracer, TracerProvider, noop::NoopTracerProvider},
    };
    let harness = Harness::new(true);
    let traces = NoopTracerProvider::new();
    let _parent = Context::current_with_span(traces.tracer("caller").start("noop")).attach();
    let observer = mf_telemetry::observation::Observer::new(&traces, &harness.logs);
    let plan = plan(success_config());
    let observation = plan.start_observation(&observer, RunId::new()).unwrap();
    execute_compiled(
        &plan,
        &NodeRegistry::from_inventory().unwrap(),
        Some(observation),
    )
    .unwrap();
    let records = harness.records();
    assert_eq!(records.len(), 7);
    for record in records {
        assert!(record.trace_context.is_none());
        record.decode().unwrap();
    }
    {
        let _suppression = Context::enter_telemetry_suppressed_scope();
        let observation = plan.start_observation(&observer, RunId::new()).unwrap();
        execute_compiled(
            &plan,
            &NodeRegistry::from_inventory().unwrap(),
            Some(observation),
        )
        .unwrap();
    }
    assert_eq!(harness.records().len(), 7);
}

#[test]
fn failures_report_real_phases_and_never_publish_a_success_first() {
    for (config, phase, visited, starts) in [
        (json!(42), "preparation", 0, 0),
        (
            json!({"ports":["value","value","inactive"]}),
            "preparation",
            0,
            0,
        ),
        (
            json!({"ports":["value","inactive"],"required":"inactive","outputs":{"value":1},"skipped":["inactive"]}),
            "publication",
            1,
            1,
        ),
        (
            json!({"ports":["value","inactive"],"outputs":{"value":1,"inactive":1},"skipped":["inactive"]}),
            "publication",
            1,
            1,
        ),
        (
            json!({"ports":["value","inactive"],"fail":true}),
            "execution",
            1,
            1,
        ),
        (
            json!({"ports":["value","inactive"],"outputs":{"undeclared":1}}),
            "publication",
            1,
            1,
        ),
        (
            json!({"ports":["value","inactive"],"outputs":{},"skipped":["inactive"]}),
            "dependency",
            3,
            1,
        ),
    ] {
        let plan = plan(config);
        let plain_error = execute_compiled(&plan, &NodeRegistry::from_inventory().unwrap(), None)
            .unwrap_err()
            .to_string();
        let (result, harness) = run(&plan, true);
        assert_eq!(result.unwrap_err().to_string(), plain_error);
        let records = harness.records();
        let last = records.last().unwrap();
        assert_eq!(last.attributes["mf.failure.phase"], phase);
        assert_eq!(last.body["visited_node_count"], visited);
        assert_eq!(
            records
                .iter()
                .filter(|r| r.event_name == "mf.node.started")
                .count(),
            starts
        );
        let failed = records
            .iter()
            .find(|r| r.event_name == "mf.node.finished" && r.attributes["mf.outcome"] == "failed")
            .unwrap();
        assert_eq!(failed.attributes["mf.failure.phase"], phase);
        assert_eq!(
            failed.body.get("duration_ns").is_some(),
            matches!(phase, "execution" | "publication")
        );
        assert_eq!(failed.body["produced_ports"], json!([]));
        let spans = harness.spans.get_finished_spans().unwrap();
        for span in spans.iter().filter(|span| {
            span.name == "mf.workflow"
                || span.span_context.span_id().to_string()
                    == failed.trace_context.as_ref().unwrap().span_id
        }) {
            assert!(matches!(
                span.status,
                opentelemetry::trace::Status::Error { .. }
            ));
            assert!(
                span.attributes
                    .iter()
                    .any(|attribute| attribute.key.as_str() == "mf.failure.phase"
                        && attribute.value.as_str() == phase)
            );
        }
        assert_eq!(
            records
                .iter()
                .filter(|r| r.event_name == "mf.node.finished"
                    && r.attributes["mf.node.id"] == failed.attributes["mf.node.id"])
                .count(),
            1
        );
    }
}

#[test]
fn output_selection_failure_preserves_prior_node_outcomes() {
    let mut plan = plan(success_config());
    plan.definition.outputs[0].node = "a".into();
    plan.definition.outputs[0].port = "inactive".into();
    plan.definition.outputs[0].optional = false;
    let (result, harness) = run(&plan, true);
    assert!(result.is_err());
    let records = harness.records();
    assert_eq!(
        records.last().unwrap().attributes["mf.failure.phase"],
        "output_selection"
    );
    assert_eq!(records.last().unwrap().body["visited_node_count"], 3);
    assert!(
        !records
            .iter()
            .any(|r| r.event_name == "mf.node.finished" && r.attributes["mf.outcome"] == "failed")
    );
}

#[test]
fn missing_dependency_takes_precedence_over_a_skip_without_invocation() {
    let mut plan = plan(json!({"ports":["value","inactive"],"skipped":["inactive"]}));
    plan.definition
        .control_edges
        .push(mf_compiler::ControlEdgeDefinition {
            from_node: "a".into(),
            from_output: "inactive".into(),
            to_node: "b".into(),
        });
    let (result, harness) = run(&plan, true);
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("missing context output")
    );
    let records = harness.records();
    assert!(!records.iter().any(|record| {
        record.event_name == "mf.node.skipped" && record.attributes["mf.node.id"] == "b"
    }));
    assert!(
        !records
            .iter()
            .any(|r| r.event_name == "mf.node.started" && r.attributes["mf.node.id"] == "b")
    );
    assert_eq!(
        records.last().unwrap().attributes["mf.failure.phase"],
        "dependency"
    );
}

#[test]
fn a_running_plugin_exposes_start_before_end_and_inherits_the_node_context() {
    use mf_runtime::{FlowNode, Inputs, NodeExecutionError, NodePorts, Outputs, TaskNode};
    use opentelemetry::{
        Context,
        trace::{Span, TraceContextExt, Tracer, TracerProvider},
    };
    use std::sync::{Mutex, mpsc};
    struct BlockingPlugin {
        tracer: opentelemetry_sdk::trace::SdkTracer,
        entered: mpsc::SyncSender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl TaskNode for BlockingPlugin {
        fn execute(
            &self,
            _: Inputs,
            _ctx: &mut mf_runtime::ExecutionContext,
        ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
            let mut child = self.tracer.start("fixture.operation");
            self.entered.send(()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            child.end();
            Ok(Outputs::new().into())
        }
    }
    let harness = Harness::new(true);
    let tracer = harness.traces.tracer("fixture.plugin");
    let parent = Context::current_with_span(tracer.start("caller"));
    let parent_id = parent.span().span_context().span_id();
    let _parent = parent.clone().attach();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let flow = mf_compiler::build_flow(
        vec![FlowNode::new(
            "blocking",
            mf_runtime::PreparedNode::new(
                BlockingPlugin {
                    tracer,
                    entered: entered_tx,
                    release: Mutex::new(release_rx),
                },
                NodePorts::default(),
            ),
        )],
        vec![],
        vec!["blocking".into()],
        vec![],
    )
    .unwrap();
    let observation = harness
        .observer()
        .start(
            mf_telemetry::identity::WorkflowId::from_definition(&json!({}), &["blocking".into()])
                .unwrap(),
            RunId::new(),
            vec![mf_telemetry::event::NodeIdentity {
                id: "blocking".into(),
                kind: "fixture.blocking".into(),
                path: Vec::new(),
            }],
        )
        .unwrap();
    std::thread::scope(|scope| {
        let worker = scope.spawn(move || flow.execute_with_observation(Some(observation)));
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(
            harness
                .records()
                .iter()
                .map(|r| r.event_name.as_str())
                .collect::<Vec<_>>(),
            ["mf.workflow.started", "mf.node.started"]
        );
        assert!(harness.spans.get_finished_spans().unwrap().is_empty());
        release_tx.send(()).unwrap();
        worker.join().unwrap().unwrap();
    });
    assert_eq!(
        Context::current().span().span_context().span_id(),
        parent_id
    );
    let spans = harness.spans.get_finished_spans().unwrap();
    let root = spans.iter().find(|s| s.name == "mf.workflow").unwrap();
    let node = spans.iter().find(|s| s.name == "mf.node").unwrap();
    let plugin = spans
        .iter()
        .find(|s| s.name == "fixture.operation")
        .unwrap();
    assert_eq!(root.parent_span_id, parent_id);
    assert_eq!(node.parent_span_id, root.span_context.span_id());
    assert_eq!(plugin.parent_span_id, node.span_context.span_id());
    assert_eq!(plugin.span_context.trace_id(), root.span_context.trace_id());
    parent.span().end();
}

#[test]
fn dropped_logs_leave_sequence_gaps_without_changing_execution_or_provider_ownership() {
    use opentelemetry::{
        InstrumentationScope,
        trace::{Span, Tracer, TracerProvider},
    };
    use opentelemetry_sdk::{
        error::OTelSdkResult,
        logs::{LogProcessor, SdkLogRecord, SdkLoggerProvider},
    };
    #[derive(Debug)]
    struct DropStart(capture::Capture);
    impl LogProcessor for DropStart {
        fn emit(&self, record: &mut SdkLogRecord, scope: &InstrumentationScope) {
            if record.event_name() != Some("mf.node.started") {
                self.0.emit(record, scope);
            }
        }
        fn force_flush(&self) -> OTelSdkResult {
            Ok(())
        }
        fn shutdown_with_timeout(&self, _: std::time::Duration) -> OTelSdkResult {
            Ok(())
        }
    }
    let harness = Harness::new(true);
    let logs = SdkLoggerProvider::builder()
        .with_log_processor(DropStart(harness.capture.clone()))
        .build();
    let observer = mf_telemetry::observation::Observer::new(&harness.traces, &logs);
    let plan = plan(success_config());
    for _ in 0..2 {
        let run = plan.start_observation(&observer, RunId::new()).unwrap();
        assert_eq!(
            json!(
                execute_compiled(&plan, &NodeRegistry::from_inventory().unwrap(), Some(run))
                    .unwrap()
            ),
            json!({"answer":2})
        );
    }
    drop(observer);
    let records = harness.records();
    assert_eq!(records.len(), 10);
    let ids: std::collections::BTreeSet<_> = records
        .iter()
        .map(|r| r.attributes["mf.run.id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    for id in ids {
        let sequence: Vec<_> = records
            .iter()
            .filter(|r| r.attributes["mf.run.id"] == id)
            .map(|r| r.decode().unwrap().sequence.get())
            .collect();
        assert_eq!(sequence.len(), 5);
        assert_eq!(sequence.first(), Some(&1));
        assert_eq!(sequence.last(), Some(&7));
        assert!(sequence.windows(2).any(|pair| pair[1] > pair[0] + 1));
    }
    let mut after = harness.traces.tracer("caller").start("after.observer.drop");
    after.end();
    assert!(
        harness
            .spans
            .get_finished_spans()
            .unwrap()
            .iter()
            .any(|s| s.name == "after.observer.drop")
    );
    logs.shutdown().unwrap();
}

#[test]
fn unwinding_restores_context_without_fabricating_completion() {
    use mf_runtime::{FlowNode, Inputs, NodeExecutionError, NodePorts, TaskNode};
    use opentelemetry::{Context, trace::TraceContextExt};
    struct PanicPlugin;
    impl TaskNode for PanicPlugin {
        fn execute(
            &self,
            _: Inputs,
            _ctx: &mut mf_runtime::ExecutionContext,
        ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
            panic!("plugin panic sentinel")
        }
    }
    let harness = Harness::new(true);
    let before = Context::current().span().span_context().clone();
    let run = harness
        .observer()
        .start(
            mf_telemetry::identity::WorkflowId::from_definition(&json!({}), &["a".into()]).unwrap(),
            RunId::new(),
            vec![mf_telemetry::event::NodeIdentity {
                id: "a".into(),
                kind: "fixture.panic".into(),
                path: Vec::new(),
            }],
        )
        .unwrap();
    let flow = mf_compiler::build_flow(
        vec![FlowNode::new(
            "a",
            mf_runtime::PreparedNode::new(PanicPlugin, NodePorts::default()),
        )],
        vec![],
        vec!["a".into()],
        vec![],
    )
    .unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        flow.execute_with_observation(Some(run))
    }));
    assert!(result.is_err());
    assert_eq!(Context::current().span().span_context(), &before);
    assert_eq!(
        harness
            .records()
            .iter()
            .map(|r| r.event_name.as_str())
            .collect::<Vec<_>>(),
        ["mf.workflow.started", "mf.node.started"]
    );
    assert_eq!(harness.spans.get_finished_spans().unwrap().len(), 2);
}

#[test]
fn preparation_errors_without_a_node_still_close_the_run() {
    let harness = Harness::new(true);
    let observer = harness.observer();
    let mut plan = plan(success_config());
    let observation = plan.start_observation(&observer, RunId::new()).unwrap();
    plan.execution_order.reverse();
    assert!(
        execute_compiled(
            &plan,
            &NodeRegistry::from_inventory().unwrap(),
            Some(observation)
        )
        .is_err()
    );
    let records = harness.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].attributes["mf.failure.phase"], "preparation");
    assert!(records[1].body.get("failure_node_id").is_none());
    assert_eq!(records[1].body["visited_node_count"], 0);
}

#[test]
fn observation_setup_rejects_bad_identity_metadata_before_emitting_a_run() {
    let harness = Harness::new(true);
    let original = plan(success_config());
    for mode in [
        "incomplete",
        "unknown",
        "duplicate_order",
        "duplicate_node",
        "blank_kind",
    ] {
        let mut plan = original.clone();
        match mode {
            "incomplete" => {
                plan.execution_order.pop();
            }
            "unknown" => plan.execution_order[0] = "missing".into(),
            "duplicate_order" => plan.execution_order[1] = plan.execution_order[0].clone(),
            "duplicate_node" => plan.definition.nodes[1].id = plan.definition.nodes[0].id.clone(),
            _ => plan.definition.nodes[0].kind.clear(),
        }
        assert!(
            plan.start_observation(&harness.observer(), RunId::new())
                .is_err(),
            "{mode}"
        );
    }
    assert!(harness.records().is_empty());
    assert!(harness.spans.get_finished_spans().unwrap().is_empty());
}

#[test]
fn an_embedded_run_closes_preparation_errors_and_restores_its_caller() {
    let harness = Harness::new(true);
    let plan = plan(success_config());
    let run = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let result = mf_runtime::ExecutionContext::run(Some(run), |state| {
        mf_compiler::instantiate_node_with_metadata(&registry, "a", "fixture.context", "{")
            .inspect_err(|error| state.preparation_failed("a", error))
            .map(|_| ())
    });
    assert!(matches!(
        result,
        Err(mf_compiler::WorkflowBuildError::InvalidEmbeddedConfig { .. })
    ));
    let records = harness.records();
    assert_eq!(records.len(), 3);
    assert_eq!(records[1].attributes["mf.failure.phase"], "preparation");
    assert_eq!(records[2].body["visited_node_count"], 0);
}

fn semantic_records(records: &[WireRecord]) -> Vec<Value> {
    let mut records: Vec<_> = records
        .iter()
        .map(|record| {
            record.decode().unwrap();
            let mut value = serde_json::to_value(record).unwrap();
            value.as_object_mut().unwrap().remove("time_unix_nano");
            value.as_object_mut().unwrap().remove("trace_context");
            value["attributes"]
                .as_object_mut()
                .unwrap()
                .remove("mf.run.id");
            value["attributes"]
                .as_object_mut()
                .unwrap()
                .remove("mf.event.sequence");
            value["body"]["elapsed_ns"] = json!(0);
            if value["body"].get("duration_ns").is_some() {
                value["body"]["duration_ns"] = json!(0);
            }
            if let Some(failure) = value["body"].get_mut("failure") {
                failure.as_object_mut().unwrap().remove("message");
            }
            value
        })
        .collect();
    records.sort_by_key(|record| serde_json::to_string(record).unwrap());
    records
}

#[test]
fn generated_execution_matches_memory_for_success_and_failures() {
    use std::{fs, process::Command};
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("build");
    for (config, output_failure) in [
        (success_config(), false),
        (json!(42), false),
        (json!({"ports":["value","inactive"],"fail":true}), false),
        (success_config(), true),
    ] {
        let mut plan = plan(config);
        plan.definition.dependencies = common::fixture_definition().dependencies;
        if output_failure {
            plan.definition.outputs[0].node = "a".into();
            plan.definition.outputs[0].port = "inactive".into();
            plan.definition.outputs[0].optional = false;
        }
        let (memory, harness) = run(&plan, true);
        mf_compiler::write_dependency_project(
            &project,
            &plan,
            &mf_compiler::SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        )
        .unwrap();
        let manifest = fs::read_to_string(project.join("Cargo.toml")).unwrap();
        fs::write(project.join("Cargo.toml"), format!(r#"{manifest}
opentelemetry = {{ version = "0.33.0", default-features = false, features = ["logs", "trace"] }}
opentelemetry_sdk = {{ version = "0.33.0", default-features = false, features = ["logs", "trace", "testing"] }}
"#)).unwrap();
        fs::write(
            project.join("src/observation_capture.rs"),
            include_str!("fixtures/observation_capture.rs"),
        )
        .unwrap();
        fs::write(project.join("src/main.rs"), r#"
extern crate node_0 as _;
mod workflow;
mod observation_capture;
fn main() {
    let plan = mf_compiler::CompiledWorkflow::from_json(include_str!("../workflow-plan.json")).unwrap();
    let harness = observation_capture::Harness::new(true);
    let observation = plan.start_observation(&harness.observer(), mf_telemetry::identity::RunId::new()).unwrap();
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let result = mf_runtime::ExecutionContext::run(Some(observation), |state| -> Result<_, Box<dyn std::error::Error>> {
        let flow = workflow::prepare_workflow(&registry, state.observation_mut())?;
        Ok(workflow::run_workflow_in_context(&flow, state)?)
    });
    println!("{}", serde_json::json!({"ok":result.is_ok(), "outputs":result.ok(), "records":harness.records(), "spans":harness.spans.get_finished_spans().unwrap().len()}));
}
"#).unwrap();
        mf_compiler::resolve_project(&project, &root.path().join("flow.lock"), false).unwrap();
        let build = mf_compiler::cargo_command(&project)
            .args(["build", "--offline", "--locked"])
            .output()
            .unwrap();
        if matches!(
            &memory,
            Err(mf_compiler::WorkflowExecutionError::Preparation { .. })
        ) {
            assert!(
                !build.status.success(),
                "invalid construction reached a runnable artifact"
            );
            continue;
        }
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let result = Command::new(common::runner_executable(&project, "debug"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["ok"], memory.is_ok());
        assert_eq!(value["outputs"], json!(memory.ok()));
        let records: Vec<WireRecord> = serde_json::from_value(value["records"].clone()).unwrap();
        assert_eq!(
            semantic_records(&records),
            semantic_records(&harness.records())
        );
        assert_eq!(
            value["spans"],
            harness.spans.get_finished_spans().unwrap().len()
        );
    }
}
