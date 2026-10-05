use mf_telemetry::{
    description::{NodeDescription, WorkflowDescription, WorkflowDescriptionVersion},
    identity::{RunId, WorkflowId},
};
use mf_tui::state::SessionState;
use serde_json::json;
use std::{hint::black_box, time::Instant};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let nodes: usize = args
        .get(1)
        .map_or(10_000, |value| value.parse().expect("node count"));
    let repetitions: usize = args
        .get(2)
        .map_or(1000, |value| value.parse().expect("repetition count"));
    assert!(repetitions > 0);
    let order: Vec<_> = (0..nodes).map(|index| format!("n{index:06}")).collect();
    let description = WorkflowDescription {
        version: WorkflowDescriptionVersion::V2026_09_27,
        workflow_id: WorkflowId::from_definition(&json!({"nodes": nodes}), &order).unwrap(),
        nodes: order
            .iter()
            .map(|id| NodeDescription {
                id: id.clone(),
                kind: "benchmark.node".into(),
            })
            .collect(),
        data_edges: Vec::new(),
        control_edges: Vec::new(),
        execution_order: order,
        execution: None,
        loop_bodies: Vec::new(),
    };
    let state = SessionState::new(description, RunId::new()).unwrap();
    let started = Instant::now();
    for _ in 0..repetitions {
        black_box(state.snapshot());
    }
    let owned_ms = started.elapsed().as_secs_f64() * 1000.0 / repetitions as f64;
    black_box(state.snapshot_shared());
    let started = Instant::now();
    for _ in 0..repetitions {
        black_box(state.snapshot_shared());
    }
    let shared_ms = started.elapsed().as_secs_f64() * 1000.0 / repetitions as f64;
    println!(
        "{}",
        json!({"nodes": nodes, "repetitions": repetitions, "owned_ms": owned_ms, "shared_ms": shared_ms})
    );
}
