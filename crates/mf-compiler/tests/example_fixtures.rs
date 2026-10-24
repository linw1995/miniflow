#![allow(dead_code)]

#[path = "../examples/benchmark_graph.rs"]
mod benchmark_graph;

#[test]
fn benchmark_graph_example_builds_a_plannable_definition() {
    let definition = benchmark_graph::definition(2, 0);
    assert_eq!(definition.nodes.len(), 2);
    assert!(mf_compiler::plan_definition(&definition).is_ok());
}
