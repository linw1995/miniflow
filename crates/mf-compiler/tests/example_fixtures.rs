#![allow(dead_code)]

#[path = "../examples/benchmark_graph.rs"]
mod benchmark_graph;
#[path = "../examples/stream.rs"]
mod stream;

#[test]
fn benchmark_graph_example_builds_a_plannable_definition() {
    let definition = benchmark_graph::definition(2, 0);
    assert_eq!(definition.nodes.len(), 2);
    assert!(mf_compiler::plan_definition(&definition).is_ok());
}

#[test]
fn stream_example_preserves_file_records_without_execution_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("records.txt");
    std::fs::write(&path, "first\n\nlast\n").unwrap();
    let mut records = Vec::new();
    stream::run(Some(path.to_string_lossy().into_owned()), |outputs| {
        records.push(serde_json::to_value(outputs)?);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        records,
        [
            serde_json::json!({"value":"first"}),
            serde_json::json!({"value":""}),
            serde_json::json!({"value":"last"}),
        ]
    );
}
