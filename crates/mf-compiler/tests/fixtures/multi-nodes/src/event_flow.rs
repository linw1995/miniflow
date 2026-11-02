use mf_runtime::{
    EventContext, EventEffects, EventNode, NodeEvent, NodeExecutionError, NodeFactory, NodePorts,
    NodeRegistration, Outputs, PortSpec, PreparedNode, ValueRef, ValueType,
};
use std::cell::Cell;

// Deliberately !Sync: every Flow invocation must own its Event state.
struct Batch {
    values: Vec<ValueRef>,
    each: bool,
    fail: bool,
    _exclusive: Cell<()>,
}

impl EventNode for Batch {
    fn on_event(
        &mut self,
        event: NodeEvent,
        _: &EventContext<'_>,
    ) -> Result<EventEffects, NodeExecutionError> {
        let flush = match event {
            NodeEvent::Input(mut inputs) => {
                if self.fail {
                    return mf_runtime::NodeExecutionFailedSnafu {
                        message: "recoverable Event failure",
                    }
                    .fail();
                }
                self.values.push(inputs.remove("item").unwrap());
                self.each
            }
            NodeEvent::UpstreamClosed => true,
            NodeEvent::Timer => false,
        };
        let emissions = if flush && !self.values.is_empty() {
            vec![
                mf_runtime::NodeResult::from(Outputs::from([(
                    "items".into(),
                    ValueRef::array(std::mem::take(&mut self.values)),
                )]))
                .into(),
            ]
        } else {
            Vec::new()
        };
        Ok(EventEffects {
            emissions,
            ..Default::default()
        })
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.flow_batch",
        factory: NodeFactory::Plain(|config| {
            Ok(PreparedNode::from_parts(
                mf_runtime::NodeExecution::Event(Box::new(Batch {
                    values: Vec::new(),
                    each: config["each"].as_bool().unwrap_or(false),
                    fail: config["fail"].as_bool().unwrap_or(false),
                    _exclusive: Cell::new(()),
                })),
                NodePorts {
                    inputs: vec![PortSpec::new("item", ValueType::Any, true)],
                    outputs: vec![PortSpec::new("items", ValueType::Array, true)],
                },
            ))
        }),
    }
}
