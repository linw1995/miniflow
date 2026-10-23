#[path = "fixtures/multi-nodes/src/task_stream.rs"]
mod fixture;

extern crate mfn_core as _;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled, instantiate_stream,
};
use mf_runtime::{StreamOptions, WorkflowArguments};
use serde_json::json;
use std::sync::{Arc, Mutex};

fn root_export() -> serde_json::Value {
    json!({
        "version": "2026-10-23", "dependencies": {},
        "nodes": [{"id": "group.count", "kind": "fixture.task_stream"}],
        "outputs": [{"name": "total", "node": "group.count", "port": "total"}],
        "stream_outputs": [{"name": "increments", "path": ["group.count"], "port": "delta"}]
    })
}

fn run_live(
    value: serde_json::Value,
    arguments: &str,
) -> (mf_runtime::FlowOutputs, Vec<mf_runtime::LiveOutputEvent>) {
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let mut context = mf_runtime::ExecutionContext::default();
    context.set_live_publisher(mf_runtime::LivePublisher::new(move |event, _| {
        captured.lock().unwrap().push(event);
        Ok(())
    }));
    context.set_workflow_arguments(WorkflowArguments::from_json(arguments.as_bytes()).unwrap());
    let result = flow.execute_in_context(&mut context).unwrap();
    let events = events.lock().unwrap().clone();
    (result, events)
}

#[test]
fn external_combined_provider_retains_terminal_execution_in_both_flow_modes() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for streaming in [false, true] {
        let mut value = json!({
            "version": "2026-10-19",
            "dependencies": {},
            "nodes": [{"id": "count", "kind": "fixture.task_stream"}],
            "outputs": [{"name": "total", "node": "count", "port": "total"}]
        });
        if streaming {
            value["execution"] = json!({"mode": "stream"});
        }
        let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let arguments = WorkflowArguments::from_json(br#"{"count.count":100}"#).unwrap();
        let output = if streaming {
            let instance = instantiate_stream(&plan, &registry)
                .unwrap()
                .start_with_options(StreamOptions {
                    arguments,
                    ..Default::default()
                })
                .unwrap();
            let output = instance.recv().unwrap().unwrap().outputs;
            assert!(instance.recv().unwrap().is_none());
            instance.join().unwrap();
            output
        } else {
            instantiate_compiled(&plan, &registry)
                .unwrap()
                .execute_with_inputs(arguments)
                .unwrap()
        };
        assert_eq!(output["total"].as_u64(), Some(100));
    }
}

#[test]
fn finite_exports_preserve_dotted_ids_and_close_empty_invocations() {
    for count in [0, 3] {
        let (output, events) = run_live(
            root_export(),
            &format!(r#"{{"group.count.count":{count}}}"#),
        );
        assert_eq!(output["total"].as_u64(), Some(count));
        assert_eq!(events.len(), count as usize + 2);
        assert!(matches!(events[0].event, mf_runtime::LiveOutputKind::Open));
        assert!(matches!(
            events.last().unwrap().event,
            mf_runtime::LiveOutputKind::Close { completed: true }
        ));
        for event in &events {
            assert_eq!(event.export, "increments");
            assert_eq!(
                event.source,
                vec![mf_runtime::DefinitionId::from("group.count")]
            );
            assert_eq!(event.invocation.node, "group.count");
            assert_eq!(event.invocation.id, events[0].invocation.id);
        }
    }
}

#[test]
fn combined_terminal_success_prerequisites_retain_ordinary_task_behavior() {
    let mut document = root_export();
    document["nodes"].as_array_mut().unwrap().push(json!({
        "id":"after", "kind":"builtin.identity", "inputs":{"input":{"from":{"node":"group.count","port":"total"}}}
    }));
    document["edges"] = json!([{"from_node":"group.count","to_node":"after"}]);
    document["outputs"] = json!([{"name":"total","node":"after","port":"value"}]);
    let (output, events) = run_live(document, r#"{"group.count.count":3}"#);
    assert_eq!(output["total"].as_u64(), Some(3));
    assert_eq!(events.len(), 5);
}

#[test]
fn export_definitions_round_trip_and_invalid_sources_fail_before_execution() {
    let definition: WorkflowDefinition = serde_json::from_value(root_export()).unwrap();
    assert_eq!(
        serde_json::from_value::<WorkflowDefinition>(serde_json::to_value(&definition).unwrap())
            .unwrap(),
        definition
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    for (field, value, expected) in [
        ("path", json!(["missing"]), "source path"),
        ("port", json!("total"), "not a stream output"),
        ("name", json!(""), "nonempty"),
    ] {
        let mut document = root_export();
        document["stream_outputs"][0][field] = value;
        let definition = serde_json::from_value(document).unwrap();
        assert!(
            compile_definition(&definition, &registry)
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
    }
    let mut duplicate = root_export();
    duplicate["stream_outputs"] = json!([
        duplicate["stream_outputs"][0],
        duplicate["stream_outputs"][0]
    ]);
    assert!(compile_definition(&serde_json::from_value(duplicate).unwrap(), &registry).is_err());
    let mut legacy = root_export();
    legacy["version"] = json!("2026-10-19");
    assert!(serde_json::from_value::<WorkflowDefinition>(legacy).is_err());
}

#[test]
fn scoped_exports_preserve_loop_and_parallel_iteration_invocations() {
    for kind in ["loop", "sequential", "parallel"] {
        let (subgraph, output, expected) = if kind == "loop" {
            (
                json!({
                    "id":"repeat", "kind":"workflow.loop", "inputs":{"count":{"from":{"node":"seed","port":"value"}}},
                    "loop":{"max_iterations":2,"variables":[{"name":"count","type":"usize"}],"body":{
                        "nodes":[{"id":"count","kind":"fixture.task_stream","inputs":{"count":{"from":{"node":"%loop","port":"count"}}}}]
                    }}
                }),
                "count",
                json!(2),
            )
        } else {
            (
                json!({
                    "id":"repeat", "kind":"builtin.iteration", "inputs":{"items":{"from":{"node":"seed","port":"value"}}},
                    "config":{"mode":kind,"body":{
                        "nodes":[{"id":"count","kind":"fixture.task_stream","inputs":{"count":{"from":{"node":"%iteration","port":"item"}}}}],
                        "result":{"node":"count","port":"total"}
                    }}
                }),
                "results",
                json!([1, 2]),
            )
        };
        let document = json!({
            "version":"2026-10-23", "dependencies":{},
            "nodes":[{"id":"seed","kind":"builtin.constant","config":{"value":if kind == "loop" {json!(2)} else {json!([1,2])}}},subgraph],
            "outputs":[{"name":"result","node":"repeat","port":output}],
            "stream_outputs":[{"name":"increments","path":["repeat","count"],"port":"delta"}]
        });
        let (result, events) = run_live(document, "{}");
        assert_eq!(serde_json::to_value(&result["result"]).unwrap(), expected);
        let opens = events
            .iter()
            .filter(|event| matches!(event.event, mf_runtime::LiveOutputKind::Open))
            .collect::<Vec<_>>();
        let closes = events
            .iter()
            .filter(|event| {
                matches!(
                    event.event,
                    mf_runtime::LiveOutputKind::Close { completed: true }
                )
            })
            .count();
        assert_eq!(opens.len(), 2);
        assert_eq!(closes, 2);
        assert_ne!(opens[0].invocation.id, opens[1].invocation.id);
        for event in opens {
            assert_eq!(event.source, vec!["repeat".into(), "count".into()]);
            assert_eq!(event.invocation.path.len(), 1);
            assert_eq!(event.invocation.path[0].loop_id, "repeat");
        }
    }
}

#[test]
fn planner_separates_terminal_and_stream_domains_and_rejects_implicit_joins() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut document = root_export();
    document["execution"] = json!({"mode":"stream"});
    document["nodes"].as_array_mut().unwrap().push(json!({
        "id":"consume","kind":"builtin.identity", "inputs":{"input":{"from":{"node":"group.count","port":"delta"}}}
    }));
    let definition = serde_json::from_value(document.clone()).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let prepared = instantiate_stream(&plan, &registry).unwrap();
    let source = prepared
        .plan()
        .nodes()
        .iter()
        .position(|node| node.definition_id.as_str() == "group.count")
        .unwrap();
    assert_eq!(prepared.plan().output_domain(source), 0);
    assert_eq!(prepared.plan().stream_output_domain(source), Some(1));
    document.as_object_mut().unwrap().remove("execution");
    assert!(
        compile_definition(
            &serde_json::from_value(document.clone()).unwrap(),
            &registry
        )
        .unwrap_err()
        .to_string()
        .contains("requires streaming execution")
    );
    document["execution"] = json!({"mode":"stream"});
    document["nodes"][1] = json!({"id":"consume","kind":"fixture.task_stream_join", "inputs":{
        "delta":{"from":{"node":"group.count","port":"delta"}},
        "total":{"from":{"node":"group.count","port":"total"}}
    }});
    let error = compile_definition(&serde_json::from_value(document).unwrap(), &registry)
        .unwrap_err()
        .to_string();
    assert!(error.contains("message domain"), "{error}");
}
