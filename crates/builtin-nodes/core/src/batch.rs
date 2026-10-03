use mf_runtime::{
    BatchInfo, EventContext, EventEffects, EventEmission, EventNode, FlushReason, NodeBuildError,
    NodeEvent, NodeExecutionError, NodeRegistration, NodeResult, OutputDerivation, Outputs,
    PortSpec, TimerUpdate, ValueRef, ValueType, deserialize_config,
};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use snafu::OptionExt;
use std::time::{Duration, Instant};

pub const KIND: &str = "builtin.batch";

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(deserialize_with = "max_items")]
    max_items: usize,
    #[serde(deserialize_with = "max_wait_ms")]
    max_wait_ms: u64,
}

fn max_items<'de, D: Deserializer<'de>>(deserializer: D) -> Result<usize, D::Error> {
    let value = usize::deserialize(deserializer)
        .map_err(|error| serde::de::Error::custom(format!("max_items: {error}")))?;
    if value == 0 {
        return Err(serde::de::Error::custom("max_items must be positive"));
    }
    Ok(value)
}

fn max_wait_ms<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = u64::deserialize(deserializer)
        .map_err(|error| serde::de::Error::custom(format!("max_wait_ms: {error}")))?;
    if value == 0
        || value.checked_mul(1_000_000).is_none()
        || Instant::now()
            .checked_add(Duration::from_millis(value))
            .is_none()
    {
        return Err(serde::de::Error::custom(
            "max_wait_ms must be positive and fit the monotonic clock",
        ));
    }
    Ok(value)
}

fn factory(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let config = deserialize_config(config)?;
    Ok(mf_runtime::PreparedNode::event(
        BatchState::new(config),
        mf_runtime::NodeMetadata {
            ports: mf_runtime::NodePorts {
                inputs: vec![PortSpec::new("item", ValueType::Any, true)],
                outputs: vec![PortSpec::new("items", ValueType::Array, true)],
            },
            output_derivations: vec![OutputDerivation::collect_input("items", "item")],
            ..Default::default()
        },
    ))
}

inventory::submit! { NodeRegistration {
    kind: KIND,
    factory: mf_runtime::NodeFactory::Plain(factory),
} }

struct BatchState {
    config: Config,
    items: Vec<ValueRef>,
    deadline: Option<Duration>,
}

impl BatchState {
    fn new(config: Config) -> Self {
        Self {
            config,
            items: Vec::new(),
            deadline: None,
        }
    }

    fn due(&self, now: Duration) -> bool {
        self.deadline.is_some_and(|deadline| deadline <= now)
    }

    fn seal(&mut self, reason: FlushReason, emissions: &mut Vec<EventEmission>) {
        if self.items.is_empty() {
            return;
        }
        let item_count = self.items.len();
        let items = ValueRef::array(std::mem::take(&mut self.items));
        self.deadline = None;
        emissions.push(EventEmission {
            result: NodeResult::from(Outputs::from([("items".into(), items)])),
            batch: Some(BatchInfo { item_count, reason }),
        });
    }
}

impl EventNode for BatchState {
    fn on_event(
        &mut self,
        event: NodeEvent,
        context: &EventContext<'_>,
    ) -> Result<EventEffects, NodeExecutionError> {
        let mut emissions = Vec::new();
        match event {
            NodeEvent::Input(mut inputs) => {
                let item = inputs
                    .remove("item")
                    .context(mf_runtime::NodeExecutionFailedSnafu {
                        message: "required input `item` was not provided",
                    })?;
                if self.due(context.now) {
                    self.seal(FlushReason::TimeoutExceed, &mut emissions);
                }
                if self.items.is_empty() {
                    let at = context
                        .now
                        .checked_add(Duration::from_millis(self.config.max_wait_ms))
                        .context(mf_runtime::NodeExecutionFailedSnafu {
                            message: "batch deadline exhausted",
                        })?;
                    self.deadline = Some(at);
                }
                self.items.push(item);
                if self.items.len() >= self.config.max_items {
                    self.seal(FlushReason::SizeExceed, &mut emissions);
                }
            }
            NodeEvent::Timer => {
                if self.due(context.now) {
                    self.seal(FlushReason::TimeoutExceed, &mut emissions);
                }
            }
            NodeEvent::UpstreamClosed => {
                let reason = if self.due(context.now) {
                    FlushReason::TimeoutExceed
                } else {
                    FlushReason::UpstreamClosed
                };
                self.seal(reason, &mut emissions);
            }
        }
        Ok(EventEffects {
            emissions,
            timer: self
                .deadline
                .map(TimerUpdate::Set)
                .unwrap_or(TimerUpdate::Cancel),
        })
    }
    fn buffered_items(&self) -> Option<usize> {
        Some(self.items.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::Inputs;
    use serde_json::json;

    fn batch(max_items: usize) -> BatchState {
        BatchState::new(Config {
            max_items,
            max_wait_ms: 100,
        })
    }
    fn input(value: Value) -> NodeEvent {
        NodeEvent::Input(Inputs::from([("item".into(), value.into())]))
    }
    fn event(batch: &mut BatchState, event: NodeEvent, milliseconds: u64) -> EventEffects {
        batch
            .on_event(
                event,
                &EventContext {
                    now: Duration::from_millis(milliseconds),
                    input: None,
                },
            )
            .unwrap()
    }
    fn values(effects: &EventEffects) -> Vec<ValueRef> {
        effects
            .emissions
            .iter()
            .map(|emission| emission.result.outputs["items"].clone())
            .collect()
    }

    #[test]
    fn configuration_requires_positive_representable_limits() {
        assert!(factory(json!({"max_items":1, "max_wait_ms":1})).is_ok());
        for field in ["max_items", "max_wait_ms"] {
            for invalid in [json!(0), json!(-1), json!(1.5), json!("1"), Value::Null] {
                let mut config = json!({"max_items":3, "max_wait_ms":100});
                config[field] = invalid;
                let error = factory(config).err().unwrap().to_string();
                assert!(error.contains(field), "{error}");
            }
            let mut config = json!({"max_items":3, "max_wait_ms":100});
            config.as_object_mut().unwrap().remove(field);
            assert!(factory(config).is_err());
        }
        assert!(factory(json!({"max_items":3, "max_wait_ms":u64::MAX})).is_err());
        assert!(factory(json!({"max_items":3, "max_wait_ms":100, "extra":1})).is_err());
    }

    #[test]
    fn count_flushes_exactly_at_the_threshold_and_retains_value_handles() {
        let mut state = batch(3);
        assert!(event(&mut state, input(json!(1)), 0).emissions.is_empty());
        assert!(event(&mut state, input(json!(2)), 10).emissions.is_empty());
        let original = state.items[0].clone();
        let effects = event(&mut state, input(json!(3)), 20);
        assert_eq!(values(&effects), [json!([1, 2, 3])]);
        assert_eq!(
            effects.emissions[0].batch.unwrap(),
            BatchInfo {
                item_count: 3,
                reason: FlushReason::SizeExceed
            }
        );
        assert!(
            effects.emissions[0].result.outputs["items"]
                .as_array()
                .unwrap()[0]
                .ptr_eq(&original)
        );
        assert!(
            event(&mut state, NodeEvent::UpstreamClosed, 30)
                .emissions
                .is_empty()
        );
        assert_eq!(
            values(&event(&mut batch(1), input(json!(null)), 0)),
            [json!([null])]
        );
    }

    #[test]
    fn timeout_is_anchored_to_the_first_item_and_does_not_need_new_input() {
        let mut state = batch(10);
        let first = event(&mut state, input(json!(1)), 10);
        assert_eq!(first.timer, TimerUpdate::Set(Duration::from_millis(110)));
        assert_eq!(event(&mut state, input(json!(2)), 100).timer, first.timer);
        let early = event(&mut state, NodeEvent::Timer, 109);
        assert!(early.emissions.is_empty());
        assert_eq!(early.timer, first.timer);
        let effects = event(&mut state, NodeEvent::Timer, 110);
        assert_eq!(values(&effects), [json!([1, 2])]);
        assert_eq!(
            effects.emissions[0].batch.unwrap().reason,
            FlushReason::TimeoutExceed
        );
        assert_eq!(effects.timer, TimerUpdate::Cancel);
    }

    #[test]
    fn deadline_ties_split_the_old_batch_before_accepting_the_new_item() {
        let mut state = batch(3);
        event(&mut state, input(json!(1)), 0);
        event(&mut state, input(json!(2)), 50);
        let effects = event(&mut state, input(json!(3)), 100);
        assert_eq!(values(&effects), [json!([1, 2])]);
        assert_eq!(
            effects.emissions[0].batch.unwrap().reason,
            FlushReason::TimeoutExceed
        );
        assert_eq!(effects.timer, TimerUpdate::Set(Duration::from_millis(200)));
        let tail = event(&mut state, NodeEvent::UpstreamClosed, 160);
        assert_eq!(values(&tail), [json!([3])]);
        assert_eq!(
            tail.emissions[0].batch.unwrap().reason,
            FlushReason::UpstreamClosed
        );
    }

    #[test]
    fn close_after_the_deadline_flushes_once_and_empty_events_do_not_emit() {
        let mut state = batch(2);
        event(&mut state, input(json!(1)), 0);
        event(&mut state, input(json!(2)), 10);
        event(&mut state, input(json!(3)), 20);
        assert!(
            event(&mut state, NodeEvent::Timer, 100)
                .emissions
                .is_empty()
        );
        let tail = event(&mut state, NodeEvent::UpstreamClosed, 120);
        assert_eq!(values(&tail), [json!([3])]);
        assert_eq!(
            tail.emissions[0].batch.unwrap().reason,
            FlushReason::TimeoutExceed
        );
        assert!(
            event(&mut state, NodeEvent::UpstreamClosed, 120)
                .emissions
                .is_empty()
        );
        assert!(
            event(&mut state, NodeEvent::Timer, 120)
                .emissions
                .is_empty()
        );
        assert!(
            event(&mut batch(2), NodeEvent::UpstreamClosed, 0)
                .emissions
                .is_empty()
        );
    }

    #[test]
    fn arrays_and_null_are_items_and_instances_have_independent_buffers() {
        let build = || match factory(json!({"max_items":3, "max_wait_ms":100}))
            .unwrap()
            .execution
        {
            mf_runtime::NodeExecution::Event(state) => state,
            _ => panic!("Batch must prepare event execution"),
        };
        let mut first = build();
        let mut second = build();
        let context = EventContext {
            now: Duration::ZERO,
            input: None,
        };
        for value in [json!([1, 2]), json!(null)] {
            first.on_event(input(value), &context).unwrap();
        }
        let effects = first.on_event(input(json!([3])), &context).unwrap();
        assert_eq!(values(&effects), [json!([[1, 2], null, [3]])]);
        assert!(
            second
                .on_event(NodeEvent::UpstreamClosed, &context)
                .unwrap()
                .emissions
                .is_empty()
        );
    }

    #[test]
    fn deadline_overflow_fails_before_accepting_the_item() {
        let mut state = batch(2);
        assert!(
            state
                .on_event(
                    input(json!(1)),
                    &EventContext {
                        now: Duration::MAX,
                        input: None
                    }
                )
                .is_err()
        );
        assert!(state.items.is_empty());
    }
}
