use mf_runtime::{
    BatchInfo, EventContext, EventEffects, EventEmission, FlushReason, Inputs, NodeEvent,
    NodeExecution, NodeExecutionError, NodeMetadata, NodeValue, PreparedNode, TimerUpdate,
    TypedEventNode, TypedNodeResult, ValueRef,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(NodeValue)]
struct Input {
    value: ValueRef,
    invalid: bool,
}

#[derive(NodeValue)]
struct Output {
    value: ValueRef,
    #[value(rename = "metric./~")]
    metric: f64,
    tag: Option<String>,
}

struct Retain {
    values: Vec<ValueRef>,
    calls: Arc<AtomicUsize>,
}

impl TypedEventNode for Retain {
    type Input = Input;
    type Output = Output;

    fn on_event(
        &mut self,
        event: NodeEvent<Input>,
        _: &EventContext<'_>,
    ) -> Result<EventEffects<Output>, NodeExecutionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (value, invalid) = match event {
            NodeEvent::Input(Input { value, invalid }) => {
                self.values.push(value.clone());
                (value, invalid)
            }
            NodeEvent::Timer => (
                self.values.last().cloned().unwrap_or_else(ValueRef::null),
                false,
            ),
            NodeEvent::UpstreamClosed => {
                self.values.clear();
                return Ok(EventEffects {
                    timer: TimerUpdate::Cancel,
                    ..Default::default()
                });
            }
        };
        let emission = |metric| EventEmission {
            result: TypedNodeResult {
                outputs: Output {
                    value: value.clone(),
                    metric,
                    tag: None,
                },
                skipped: ["tag".into()].into(),
                loop_summary: None,
            },
            batch: Some(BatchInfo {
                item_count: 1,
                reason: FlushReason::TimeoutExceed,
            }),
        };
        Ok(EventEffects {
            emissions: vec![
                emission(1.0),
                emission(if invalid { f64::NAN } else { 2.0 }),
            ],
            timer: TimerUpdate::Set(Duration::from_millis(13)),
        })
    }

    fn buffered_items(&self) -> Option<usize> {
        Some(self.values.len())
    }
}

#[test]
fn typed_events_decode_before_dispatch_and_encode_all_emissions_with_control_metadata() {
    let calls = Arc::new(AtomicUsize::new(0));
    let prepared = PreparedNode::typed_event(
        Retain {
            values: vec![],
            calls: calls.clone(),
        },
        NodeMetadata::default(),
    )
    .unwrap();
    assert_eq!(prepared.metadata.ports.inputs, Input::ports());
    assert_eq!(prepared.metadata.ports.outputs, Output::ports());
    let NodeExecution::Event(mut node) = prepared.execution else {
        panic!("expected event execution")
    };
    let context = EventContext {
        now: Duration::ZERO,
        input: None,
    };
    let error = node
        .on_event(NodeEvent::Input(Inputs::new()), &context)
        .unwrap_err();
    let NodeExecutionError::InputDecode { source } = error else {
        panic!("expected typed input error")
    };
    assert_eq!(source.pointer(), "/value");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(node.buffered_items(), Some(0));
    assert!(
        node.on_event(NodeEvent::Timer, &context).unwrap().emissions[0]
            .result
            .outputs["value"]
            .is_null()
    );
    let value = ValueRef::from(serde_json::json!({"nested":[1,2]}));
    let effects = node
        .on_event(
            NodeEvent::Input(Inputs::from([
                ("value".into(), value.clone()),
                ("invalid".into(), false.into()),
            ])),
            &context,
        )
        .unwrap();
    assert_eq!(node.buffered_items(), Some(1));
    assert_eq!(effects.timer, TimerUpdate::Set(Duration::from_millis(13)));
    for emission in effects.emissions {
        assert!(emission.result.outputs["value"].ptr_eq(&value));
        assert_eq!(emission.result.skipped, ["tag".into()].into());
        assert!(!emission.result.outputs.contains_key("tag"));
        assert_eq!(emission.batch.unwrap().item_count, 1);
    }
    let error = node
        .on_event(
            NodeEvent::Input(Inputs::from([
                ("value".into(), value),
                ("invalid".into(), true.into()),
            ])),
            &context,
        )
        .unwrap_err();
    let NodeExecutionError::OutputEncode { source } = error else {
        panic!("expected typed output error")
    };
    assert_eq!(source.pointer(), "/metric.~1~0");
    let close = node.on_event(NodeEvent::UpstreamClosed, &context).unwrap();
    assert!(close.emissions.is_empty());
    assert_eq!(close.timer, TimerUpdate::Cancel);
    assert_eq!(node.buffered_items(), Some(0));
}
