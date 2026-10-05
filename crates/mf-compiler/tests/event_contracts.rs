use mf_runtime::{
    EventContext, EventEffects, EventNode, FlowNode, NodeBuildError, NodeEvent, NodeExecutionError,
    NodeFactory, NodePorts, NodeRegistration, PreparedNode,
};
use serde_json::Value;
use std::cell::Cell;

struct EventState {
    _not_sync: Cell<()>,
}

impl EventNode for EventState {
    fn on_event(
        &mut self,
        _: NodeEvent,
        _: &EventContext<'_>,
    ) -> Result<EventEffects, NodeExecutionError> {
        panic!("synchronous preparation must not invoke events")
    }
}

fn factory(_: Value) -> Result<PreparedNode, NodeBuildError> {
    Ok(PreparedNode::event(
        EventState {
            _not_sync: Cell::new(()),
        },
        NodePorts::default(),
    ))
}

#[test]
fn prepares_send_only_event_state_and_rejects_synchronous_execution() {
    let registration = NodeRegistration {
        kind: "test.event",
        factory: NodeFactory::Plain(factory),
    };
    let prepared = registration.instantiate(Value::Null).unwrap();
    assert!(prepared.execution.as_task_node().is_none());
    let error = mf_compiler::build_flow(
        vec![FlowNode::new("collector", prepared)],
        Vec::new(),
        vec!["collector".into()],
        Vec::new(),
    )
    .err()
    .expect("an event cannot enter a synchronous flow");
    assert!(
        matches!(error, mf_compiler::FlowBuildError::NonTaskNode { definition_id }
        if definition_id.as_str() == "collector")
    );
}
