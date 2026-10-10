use mf_runtime::{
    EventContext, EventEffects, EventNode, NodeBuildError, NodeEvent, NodeExecutionError,
    NodeFactory, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, PreparedNode,
    TimerUpdate, ValueRef, ValueType,
};
use serde_json::Value;
use std::{cell::Cell, time::Duration};

struct Collector {
    values: Vec<ValueRef>,
    max_items: usize,
    wait: Duration,
    _not_sync: Cell<()>,
}

impl EventNode for Collector {
    fn on_event(
        &mut self,
        event: NodeEvent,
        context: &EventContext<'_>,
    ) -> Result<EventEffects, NodeExecutionError> {
        if let NodeEvent::Input(mut inputs) = event {
            self.values.push(inputs.remove("item").unwrap());
            if self.values.len() < self.max_items {
                return Ok(EventEffects {
                    emissions: Vec::new(),
                    timer: if self.values.len() == 1 {
                        TimerUpdate::Set(context.now + self.wait)
                    } else {
                        TimerUpdate::Keep
                    },
                });
            }
        }
        let emissions = if self.values.is_empty() {
            Vec::new()
        } else {
            vec![
                NodeResult::from(Outputs::from([(
                    "items".into(),
                    ValueRef::array(std::mem::take(&mut self.values)),
                )]))
                .into(),
            ]
        };
        Ok(EventEffects {
            emissions,
            timer: TimerUpdate::Cancel,
        })
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    Ok(PreparedNode::from_parts(
        mf_runtime::NodeExecution::Event(Box::new(Collector {
            values: Vec::new(),
            max_items: config["max_items"]
                .as_u64()
                .map_or(usize::MAX, |n| n as usize),
            wait: Duration::from_millis(config["max_wait_ms"].as_u64().unwrap_or(100)),
            _not_sync: Cell::new(()),
        })),
        NodePorts {
            inputs: vec![PortSpec::new("item", ValueType::Any, true)],
            outputs: vec![PortSpec::new("items", ValueType::Array, true)],
        },
    ))
}

inventory::submit! {
    NodeRegistration { kind: "test.accumulate", factory: NodeFactory::Plain(factory) }
}
