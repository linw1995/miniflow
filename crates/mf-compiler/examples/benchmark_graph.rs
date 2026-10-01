extern crate mfn_core as _;

use mf_compiler::{instantiate_compiled, plan_definition};
use mf_runtime::{
    EdgeDefinition, NodeDefinition, NodeRegistry, WorkflowDefinition, WorkflowDefinitionVersion,
    WorkflowOutputDefinition,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, hint::black_box, time::Instant};

fn definition(count: usize, payload: usize) -> WorkflowDefinition {
    assert!(count > 0);
    let id = |index| format!("n{index:06}");
    let value = if payload == 0 {
        json!(42)
    } else {
        Value::String("x".repeat(payload))
    };
    let mut nodes = vec![NodeDefinition {
        id: id(0).into(),
        kind: "builtin.constant".into(),
        config: json!({ "value": value }),
        loop_definition: None,
    }];
    let mut edges = Vec::new();
    for index in 1..count {
        nodes.push(NodeDefinition {
            id: id(index).into(),
            kind: "builtin.identity".into(),
            config: json!({}),
            loop_definition: None,
        });
        edges.push(EdgeDefinition {
            from_node: id(index - 1).into(),
            from_output: "value".into(),
            to_node: id(index).into(),
            to_input: "input".into(),
        });
    }
    WorkflowDefinition {
        version: WorkflowDefinitionVersion::CURRENT,
        dependencies: BTreeMap::new(),
        nodes,
        edges,
        control_edges: Vec::new(),
        outputs: vec![WorkflowOutputDefinition {
            name: "result".into(),
            node: id(count - 1).into(),
            port: "value".into(),
            optional: false,
        }],
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let count = args
        .get(1)
        .map_or(1000, |value| value.parse().expect("node count"));
    let payload = args
        .get(2)
        .map_or(0, |value| value.parse().expect("payload bytes"));
    let definition = definition(count, payload);
    let registry = NodeRegistry::from_inventory().unwrap();
    let started = Instant::now();
    let plan = plan_definition(&definition).unwrap();
    let plan_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    black_box(flow.execute().unwrap());
    let execute_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let source_bytes = black_box(plan.generate_artifacts().unwrap())
        .rust_source
        .len();
    let generate_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{}",
        json!({
            "nodes": count, "payload_bytes": payload, "plan_ms": plan_ms,
            "prepare_ms": prepare_ms, "execute_ms": execute_ms,
            "generate_ms": generate_ms, "source_bytes": source_bytes,
        })
    );
}
