#[path = "fixtures/task-stream/src/lib.rs"]
mod fixture;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled, instantiate_stream,
};
use mf_runtime::{StreamOptions, WorkflowArguments};
use serde_json::json;

#[test]
fn registered_external_provider_delivers_increments_and_one_terminal_result() {
    struct Collect(Vec<u64>);
    impl mf_runtime::EmissionSink for Collect {
        fn send(&mut self, result: mf_runtime::NodeResult) -> Result<(), mf_runtime::StreamError> {
            self.0.push(result.outputs["delta"].as_u64().unwrap());
            Ok(())
        }
    }
    let registry = NodeRegistry::from_inventory().unwrap();
    let prepared = registry
        .get("fixture.task_stream")
        .unwrap()
        .instantiate(json!({}))
        .unwrap();
    let mf_runtime::NodeExecution::TaskStream(executor) = prepared.execution else {
        panic!("expected a combined executor");
    };
    let mut sink = Collect(Vec::new());
    let result = executor
        .execute_with_sink(
            [("count".into(), 3_u64.into())].into(),
            &mut mf_runtime::ExecutionContext::default(),
            &mut sink,
        )
        .unwrap();
    assert_eq!(sink.0, [0, 1, 2]);
    assert_eq!(result.outputs["total"].as_u64(), Some(3));
    assert!(!result.outputs.contains_key("delta"));
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
fn combined_terminal_results_support_output_and_success_dependencies() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-10-23", "dependencies": {},
        "nodes": [
            {"id":"source", "kind":"fixture.task_stream"},
            {"id":"after", "kind":"fixture.task_stream", "inputs": {
                "count":{"from":{"node":"source", "port":"total"}}
            }}
        ],
        "edges": [{"from_node":"source", "to_node":"after"}],
        "outputs": [{"name":"total", "node":"after", "port":"total"}]
    }))
    .unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let outputs = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_with_inputs(WorkflowArguments::from_json(br#"{"source.count":3}"#).unwrap())
        .unwrap();
    assert_eq!(outputs["total"].as_u64(), Some(3));
}
