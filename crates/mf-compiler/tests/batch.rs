#[path = "fixtures/controlled_source.rs"]
mod controlled;
use controlled::SourceRun;
extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use mf_runtime::{StreamClock, StreamOptions, ValueType};
use serde_json::json;
use std::{sync::Arc, time::Duration};

struct FixedClock;
impl StreamClock for FixedClock {
    fn now(&self) -> Duration {
        Duration::ZERO
    }
}

fn definition() -> WorkflowDefinition {
    let mut definition =
        WorkflowDefinition::from_json(include_str!("../../../examples/stream-batch.json")).unwrap();
    definition.dependencies.remove("code");
    definition
        .nodes
        .retain(|node| node.id.as_str() != "convert");
    let input = definition
        .edges
        .iter_mut()
        .find(|edge| edge.to_node.as_str() == "convert")
        .unwrap();
    input.to_node = "collect".into();
    input.to_input = "item".into();
    definition
        .edges
        .retain(|edge| edge.from_node.as_str() != "convert");
    let source = definition
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "feed")
        .unwrap();
    source.kind = "test.controlled_source".into();
    source.config = json!({"item_type":"int"});
    definition
        .edges
        .iter_mut()
        .find(|edge| edge.from_node.as_str() == "feed")
        .unwrap()
        .from_output = "item".into();
    definition
}

#[test]
fn batch_infers_types_and_delivers_full_and_tail_batches() {
    let definition = definition();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let prepared = instantiate_stream(&plan, &registry).unwrap();
    assert_eq!(
        prepared.plan().nodes()[1].metadata.ports.outputs[0].value_type,
        ValueType::List(Box::new(ValueType::Int64))
    );
    let instance = SourceRun::start(
        prepared,
        "feed",
        StreamOptions {
            clock: Arc::new(FixedClock),
            ..StreamOptions::default()
        },
    )
    .unwrap();
    for value in 1..=5 {
        instance.source.clone().send(json!(value)).unwrap();
    }
    instance.source.close();
    assert_eq!(
        instance.recv().unwrap().unwrap().outputs["batch"],
        json!([1, 2, 3])
    );
    assert_eq!(
        instance.recv().unwrap().unwrap().outputs["batch"],
        json!([4, 5])
    );
    assert!(instance.recv().unwrap().is_none());
    assert_eq!(instance.join().unwrap().emitted_messages, 7);
}

#[test]
fn conditional_skips_do_not_add_items_or_skip_a_pending_batch() {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-03", "execution":{"mode":"stream"}, "dependencies":{},
        "nodes":[
            {"id":"route", "kind":"builtin.if_else", "config":{"branches":[{"id":"positive", "condition":{
                "source":{"output":"feed.item", "path":""}, "operator":"gt", "value":0
            }}]}},
            {"id":"collect", "kind":"builtin.batch", "config":{"max_items":3, "max_wait_ms":100}},
            controlled::source(json!("int"))
        ],
        "edges":[{"from_node":"feed", "from_output":"item", "to_node":"collect", "to_input":"item"}],
        "control_edges":[{"from_node":"feed", "from_output":"item", "to_node":"route"},
            {"from_node":"route", "from_output":"positive", "to_node":"collect"}],
        "outputs":[{"name":"batch", "node":"collect", "port":"items"}]
    })).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    for (input, expected) in [(vec![1, -1, 2], Some(json!([1, 2]))), (vec![-1, -2], None)] {
        let instance = SourceRun::start(
            instantiate_stream(&plan, &registry).unwrap(),
            "feed",
            StreamOptions {
                clock: Arc::new(FixedClock),
                ..StreamOptions::default()
            },
        )
        .unwrap();
        for value in input {
            instance.source.clone().send(json!(value)).unwrap();
        }
        instance.source.close();
        if let Some(expected) = expected {
            assert_eq!(instance.recv().unwrap().unwrap().outputs["batch"], expected);
        }
        assert!(instance.recv().unwrap().is_none());
        instance.join().unwrap();
    }
}

#[test]
fn a_batch_provider_must_be_linked_explicitly() {
    let definition = definition();
    let error = compile_definition(&definition, &NodeRegistry::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("builtin.batch"), "{error}");
}
