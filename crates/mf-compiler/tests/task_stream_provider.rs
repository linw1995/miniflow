#[path = "fixtures/task-stream/src/lib.rs"]
mod fixture;

use mf_compiler::NodeRegistry;
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
