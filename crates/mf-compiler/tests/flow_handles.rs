mod common;
#[path = "fixtures/multi-nodes/src/event_flow.rs"]
mod event_fixture;
#[path = "fixtures/multi-nodes/src/task_stream.rs"]
mod stream_fixture;
extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use mf_runtime::{StreamOptions, WorkflowArguments};
use serde_json::{Value, json};

fn event_flow(config: Value, each: bool) -> Value {
    json!({
        "version":"2026-11-01", "dependencies":{},
        "nodes":[
            {"id":"step", "kind":"fixture.task_stream", "config":config},
            {"id":"batch", "kind":"fixture.flow_batch", "config":{"each":each},
             "inputs":{"item":{"from":{"node":"step", "port":"delta"}}}},
            {"id":"copy", "kind":"builtin.identity",
             "inputs":{"input":{"from":{"node":"batch", "port":"items"}}}}
        ],
        "outputs":[{"name":"batch", "node":"copy", "port":"value"}]
    })
}

fn call(body: Value) -> Value {
    json!({
        "version":"2026-11-01", "dependencies":{},
        "nodes":[{"id":"invoke", "kind":"workflow.call", "config":{"flow":"child", "definition":body}}],
        "outputs":[{"name":"batch", "node":"invoke", "port":"batch"}]
    })
}

fn reference(body: Value) -> Value {
    json!({
        "version":"2026-11-01", "dependencies":{},
        "flow_arguments":{"child":{"definition":body, "description":null, "parameters":{}}},
        "nodes":[
            {"id":"select", "kind":"builtin.constant", "config":{"value":"child"}},
            {"id":"args", "kind":"builtin.constant", "config":{"value":{"step.count":2}}},
            {"id":"invoke", "kind":"workflow.call", "config":{
                "flow_ref":{"$ref":"select.value"}, "arguments_object":true, "capture_errors":true
            }, "inputs":{"arguments":{"from":{"node":"args", "port":"value"}}}}
        ],
        "edges":[{"from_node":"select", "to_node":"invoke"}],
        "outputs":[{"name":"batch", "node":"invoke", "port":"batch"},
                   {"name":"completion", "node":"invoke", "port":"result"}]
    })
}

fn run(value: Value, arguments: Value) -> Vec<Value> {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let frozen =
        WorkflowDefinition::from_json(&serde_json::to_string(&plan.definition).unwrap()).unwrap();
    assert_eq!(
        compile_definition(&frozen, &registry).unwrap().definition,
        plan.definition
    );
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            arguments: WorkflowArguments::try_from(arguments).unwrap(),
            ..Default::default()
        })
        .unwrap();
    let mut values = Vec::new();
    while let Some(output) = instance.recv().unwrap() {
        values.push(serde_json::to_value(output.outputs).unwrap());
    }
    instance.join().unwrap();
    values
}

#[test]
fn static_and_passed_event_flows_flush_and_end_without_a_terminal_business_value() {
    for nested in [false, true] {
        let body = call(event_flow(json!({}), false));
        let (value, arguments) = if nested {
            (call(body), json!({"invoke.invoke.step.count":2}))
        } else {
            (body, json!({"invoke.step.count":2}))
        };
        assert_eq!(run(value, arguments), vec![json!({"batch":[0,1]})]);
    }
    let values = run(reference(event_flow(json!({}), false)), json!({}));
    assert_eq!(values.len(), 2);
    assert!(values.contains(&json!({"batch":[0,1]})));
    assert!(values.contains(&json!({"completion":{"outputs":{}}})));
}

#[derive(Default)]
struct Collect(Vec<Value>);
impl mf_runtime::EmissionSink for Collect {
    fn send(&mut self, result: mf_runtime::NodeResult) -> Result<(), mf_runtime::StreamError> {
        self.0.push(serde_json::to_value(result.outputs).unwrap());
        Ok(())
    }
}

#[test]
fn repeated_and_concurrent_calls_own_fresh_event_state_and_keep_terminal_results_empty() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = call(event_flow(json!({}), false));
    value["outputs"] = json!([]);
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let flow = mf_compiler::instantiate_compiled(&plan, &registry).unwrap();
    let invoke = || {
        let mut sink = Collect::default();
        let result = flow.nodes()[0]
            .node
            .execute_with_sink(
                [("step.count".into(), 2_u64.into())].into(),
                &mut mf_runtime::ExecutionContext::default(),
                &mut sink,
            )
            .unwrap();
        assert!(result.outputs.is_empty());
        assert_eq!(sink.0, vec![json!({"batch":[0,1]})]);
    };
    invoke();
    invoke();
    std::thread::scope(|scope| {
        let first = scope.spawn(invoke);
        let second = scope.spawn(invoke);
        first.join().unwrap();
        second.join().unwrap();
    });
}

#[test]
fn receiving_call_can_capture_an_event_failure_without_cancelling_its_parent() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut body = event_flow(json!({}), false);
    body["nodes"][1]["config"]["fail"] = json!(true);
    let mut value = reference(body);
    value["outputs"]
        .as_array_mut()
        .unwrap()
        .retain(|output| output["name"] == "completion");
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let flow = mf_compiler::instantiate_compiled(&plan, &registry).unwrap();
    let mut context = mf_runtime::ExecutionContext::default();
    let outputs = flow.execute_in_context(&mut context).unwrap();
    assert!(
        outputs["completion"]["error"]
            .as_str()
            .unwrap()
            .contains("recoverable Event failure")
    );
    assert!(context.cancellation().failure().is_none());
}

#[test]
fn independent_event_output_domains_are_sparse_packets_at_the_handle_boundary() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut body = event_flow(json!({}), false);
    body["outputs"].as_array_mut().unwrap().push(json!({
        "name":"count", "node":"step", "port":"total"
    }));
    let mut value = call(body);
    value["outputs"] = json!([]);
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let flow = mf_compiler::instantiate_compiled(&plan, &registry).unwrap();
    assert!(
        flow.nodes()[0]
            .metadata
            .stream_outputs
            .iter()
            .all(|port| !port.required)
    );
    let mut sink = Collect::default();
    let result = flow.nodes()[0]
        .node
        .execute_with_sink(
            [("step.count".into(), 2_u64.into())].into(),
            &mut mf_runtime::ExecutionContext::default(),
            &mut sink,
        )
        .unwrap();
    assert!(result.outputs.is_empty());
    assert_eq!(sink.0.len(), 2);
    assert!(sink.0.contains(&json!({"batch":[0,1]})));
    assert!(sink.0.contains(&json!({"count":2})));
}

#[test]
fn nested_event_messages_keep_the_call_scope_and_snapshot_recorder() {
    use std::sync::{Arc, Mutex};
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = call(event_flow(json!({}), true));
    value["outputs"] = json!([]);
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let events = Arc::new(Mutex::new(Vec::<mf_runtime::SnapshotEvent>::new()));
    let captured = events.clone();
    let mut context = mf_runtime::ExecutionContext::default();
    context.set_workflow_arguments(
        WorkflowArguments::try_from(json!({"invoke.step.count":2})).unwrap(),
    );
    context.set_snapshot_recorder(mf_runtime::SnapshotRecorder::new(move |event| {
        captured.lock().unwrap().push(event);
    }));
    mf_compiler::instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_in_context(&mut context)
        .unwrap();
    let events = events.lock().unwrap();
    for node in ["step", "batch", "copy"] {
        let selected: Vec<_> = events
            .iter()
            .filter(|event| event.node.as_ref() == node)
            .collect();
        assert!(!selected.is_empty(), "missing nested snapshots for {node}");
        assert!(selected.iter().all(|event| event.scope
            == vec![mf_telemetry::event::ScopePathEntry::Call {
                call_id: "invoke".into()
            }]));
    }
    assert_eq!(
        events
            .iter()
            .filter(|event| event.node.as_ref() == "copy"
                && event.snapshot.outcome == mf_runtime::SnapshotOutcome::Succeeded)
            .count(),
        2
    );
}

#[test]
fn parent_cancellation_reaches_an_event_flow_after_incremental_output() {
    let root = tempfile::tempdir().unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let value = call(event_flow(
        json!({"wait_path":root.path().join("unreleased")}),
        true,
    ));
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            arguments: WorkflowArguments::try_from(json!({"invoke.step.count":2})).unwrap(),
            ..Default::default()
        })
        .unwrap();
    let first = instance.receive().unwrap().unwrap();
    assert_eq!(first.output().outputs["batch"], json!([0]));
    first.acknowledge().unwrap();
    instance.cancellation().cancel(
        mf_runtime::StreamExecutionSnafu {
            message: "cancel nested Event Flow",
        }
        .build(),
    );
    assert!(instance.recv().is_err());
    assert!(instance.join().is_err());
}

#[cfg(unix)]
#[test]
fn standalone_nested_passed_event_flows_execute_frozen_programs_without_source_files() {
    use std::{fs, process::Command};
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("flow.json");
    let output = root.path().join("runner");
    let build = root.path().join("build");
    let marker = root.path().join("executed");
    let mut value = call(reference(event_flow(json!({"marker":marker}), false)));
    value["outputs"].as_array_mut().unwrap().push(json!({
        "name":"completion", "node":"invoke", "port":"completion"
    }));
    value["dependencies"] = json!({
        "core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")},
        "fixture":{"package":"fixture-multi-nodes", "path":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes")}
    });
    fs::write(&source, value.to_string()).unwrap();
    mf_compiler::compile_project_with_options(
        &mf_compiler::CompileRequest {
            definition: &source,
            output: &output,
            locked: false,
            build_dir: Some(&build),
            support: &mf_compiler::SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        },
        &mf_compiler::RunnerOptions { telemetry: false },
    )
    .unwrap();
    fs::remove_file(source).unwrap();
    fs::remove_dir_all(build).unwrap();
    let drift = Command::new(&output)
        .env_clear()
        .env("PATH", "")
        .env("MF_FIXTURE_STREAM_CONTRACT_CHANGE", "1")
        .output()
        .unwrap();
    assert!(!drift.status.success());
    assert!(
        String::from_utf8_lossy(&drift.stderr).contains("prepared metadata differs"),
        "{}",
        String::from_utf8_lossy(&drift.stderr)
    );
    assert!(drift.stdout.is_empty());
    assert!(!marker.exists());
    let result = Command::new(output)
        .env_clear()
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let values: Vec<Value> = serde_json::Deserializer::from_slice(&result.stdout)
        .into_iter()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(values.len(), 2);
    assert!(values.contains(&json!({"batch":[0,1]})));
    // The receiver's completion envelope is separate from the Event batch;
    // it is never synthesized by collecting or selecting an emission.
    assert!(values.contains(&json!({"completion":{"outputs":{}}})));
}
