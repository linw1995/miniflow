#![allow(dead_code)]

#[path = "../examples/benchmark_routes.rs"]
mod benchmark_routes;
#[path = "../examples/benchmark_snapshots.rs"]
mod benchmark_snapshots;
#[path = "../examples/graph_preview.rs"]
mod graph_preview;

#[test]
fn benchmark_and_preview_examples_construct_current_descriptions() {
    let routes = benchmark_routes::description(2);
    assert_eq!(routes.nodes.len(), 2);

    let snapshots = benchmark_snapshots::description(2);
    assert_eq!(snapshots.nodes.len(), 2);

    let preview = graph_preview::description();
    let body = graph_preview::body_description(&preview);
    assert_eq!(body.nodes.len(), preview.nodes.len());
}
