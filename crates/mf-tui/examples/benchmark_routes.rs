//! Deterministic routing fixtures with work counters and wall-clock measurements.

use mf_telemetry::{
    description::{
        ControlEdge, DataEdge, NodeDescription, WorkflowDescription, WorkflowDescriptionVersion,
    },
    identity::WorkflowId,
};
use mf_tui::graph::GraphLayout;
use std::time::Instant;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let mode = arguments.next().unwrap_or_else(|| "chain".into());
    let size: usize = arguments
        .next()
        .as_deref()
        .unwrap_or("256")
        .parse()
        .expect("size must be an integer");
    assert!(size >= 2);
    let mut graph = WorkflowDescription {
        order_edges: Vec::new(),
        version: WorkflowDescriptionVersion::V2026_09_27,
        workflow_id: WorkflowId::try_from(format!("sha256:{}", "d".repeat(64))).unwrap(),
        nodes: (0..size)
            .map(|index| NodeDescription {
                id: format!("node-{index:05}"),
                kind: "fixture.node".into(),
            })
            .collect(),
        execution_order: (0..size).map(|index| format!("node-{index:05}")).collect(),
        data_edges: Vec::new(),
        control_edges: Vec::new(),
        execution: None,
        loop_bodies: Vec::new(),
        clusters: Vec::new(),
    };
    let edge = |from: usize, to: usize, port: usize| DataEdge {
        exclusive: false,
        from_node: format!("node-{from:05}"),
        from_output: "value".into(),
        to_node: format!("node-{to:05}"),
        to_input: format!("input-{port}"),
    };
    match mode.as_str() {
        "chain" => graph
            .data_edges
            .extend((1..size).map(|to| edge(to - 1, to, 0))),
        "fan" => graph.data_edges.extend((1..size).map(|to| edge(0, to, 0))),
        "join" => graph
            .data_edges
            .extend((0..size - 1).map(|from| edge(from, size - 1, from))),
        "parallel" => {
            graph.nodes.truncate(2);
            graph.execution_order.truncate(2);
            graph
                .data_edges
                .extend((0..size).map(|port| edge(0, 1, port)));
            graph
                .control_edges
                .extend((0..size).map(|port| ControlEdge {
                    from_node: "node-00000".into(),
                    from_output: format!("gate-{port}"),
                    to_node: "node-00001".into(),
                }));
        }
        "dense" => {
            let width = size / 3;
            for from in 0..width {
                for to in width..width * 3 {
                    graph.data_edges.push(edge(from, to, from));
                }
            }
            for from in width..width * 2 {
                for to in width * 2..width * 3 {
                    graph.data_edges.push(edge(from, to, from));
                }
            }
        }
        _ => panic!("mode must be chain, fan, join, parallel, or dense"),
    }
    let started = Instant::now();
    let layout = GraphLayout::new(&graph).unwrap();
    let elapsed = started.elapsed();
    let stats = layout.routing_stats();
    println!(
        "mode,nodes,edges,milliseconds,unresolved,crossings,width,height,geometry_items,search_expansions,conflict_checks,retry_rounds"
    );
    println!(
        "{mode},{},{},{:.3},{},{},{},{},{},{},{},{}",
        layout.nodes().len(),
        layout.edges().len(),
        elapsed.as_secs_f64() * 1000.0,
        layout
            .routes()
            .iter()
            .filter(|route| route.unresolved.is_some())
            .count(),
        layout.crossings().len(),
        layout.size().0,
        layout.size().1,
        stats.geometry_items,
        stats.search_expansions,
        stats.conflict_checks,
        stats.retry_rounds
    );
}
