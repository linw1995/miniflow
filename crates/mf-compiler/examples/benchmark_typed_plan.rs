//! Run with `cargo run --release -p mf-compiler --example benchmark_typed_plan -- 4096 20`.
//! Flow preparation is timed separately so other compilation passes do not mask planning costs.

extern crate mfn_core as _;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled, plan_typed_segments,
};
use serde_json::{Value, json};
use std::{hint::black_box, time::Instant};

pub fn definition(count: usize, control_reads: bool) -> WorkflowDefinition {
    assert!(count > 0);
    let id = |index| format!("n{index:06}");
    let nodes: Vec<_> = (0..count)
        .map(|index| {
            let mut node = json!({"id": id(index), "kind": "builtin.identity"});
            if index > 0 {
                node["inputs"] =
                    json!({"input": {"from": {"node": id(index - 1), "port": "value"}}});
            }
            node
        })
        .collect();
    let control_edges: Vec<_> = (1..count)
        .filter(|_| control_reads)
        .map(|index| {
            json!({"from_node": id(index - 1), "from_output": "value", "to_node": id(index)})
        })
        .collect();
    serde_json::from_value(json!({
        "version": mf_runtime::WorkflowDefinitionVersion::CURRENT,
        "dependencies": {}, "nodes": nodes, "edges": control_edges,
        "outputs": [{"name": "result", "node": id(count - 1), "port": "value"}],
    }))
    .unwrap()
}

pub fn run(args: &[String], mut report: impl FnMut(Value)) {
    let count = args
        .first()
        .map_or(4096, |value| value.parse().expect("node count"));
    let iterations = args
        .get(1)
        .map_or(20, |value| value.parse().expect("iteration count"));
    assert!(iterations > 0);
    let registry = NodeRegistry::from_inventory().unwrap();
    for control_reads in [false, true] {
        let definition = definition(count, control_reads);
        let started = Instant::now();
        let compiled = compile_definition(&definition, &registry).unwrap();
        let flow = instantiate_compiled(&compiled, &registry).unwrap();
        let prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
        let expected = plan_typed_segments(&flow, true, true);
        assert_eq!(expected.connections.len(), count - 1);
        assert_eq!(
            expected.segments.len(),
            usize::from(count >= 2 && !control_reads)
        );
        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let started = Instant::now();
            let plan = plan_typed_segments(black_box(&flow), true, true);
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(black_box(plan), expected);
        }
        samples.sort_by(f64::total_cmp);
        report(json!({
            "nodes": count, "control_reads": control_reads, "iterations": iterations,
            "prepare_ms": prepare_ms, "typed_plan_median_ms": samples[iterations / 2],
            "segments": expected.segments.len(), "connections": expected.connections.len(),
        }));
    }
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    run(&args, |report| println!("{report}"));
}
