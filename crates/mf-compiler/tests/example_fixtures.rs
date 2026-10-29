#![allow(dead_code)]

#[path = "../examples/benchmark_graph.rs"]
mod benchmark_graph;
#[cfg(feature = "codegen")]
#[path = "../examples/benchmark_typed_plan.rs"]
mod benchmark_typed_plan;
#[path = "../examples/stream.rs"]
mod stream;

#[test]
fn benchmark_graph_example_builds_a_plannable_definition() {
    let definition = benchmark_graph::definition(2, 0);
    assert_eq!(definition.nodes.len(), 2);
    assert!(mf_compiler::plan_definition(&definition).is_ok());
}

#[cfg(feature = "codegen")]
#[test]
fn typed_plan_benchmark_covers_chains_and_control_reads() {
    let registry = mf_compiler::NodeRegistry::from_inventory().unwrap();
    for control_reads in [false, true] {
        let definition = benchmark_typed_plan::definition(4, control_reads);
        let compiled = mf_compiler::compile_definition(&definition, &registry).unwrap();
        let flow = mf_compiler::instantiate_compiled(&compiled, &registry).unwrap();
        let plan = mf_compiler::plan_typed_segments(&flow, true, true);
        assert_eq!(flow.execution_domains().len(), 1);
        assert_eq!(plan.connections.len(), 3);
        if control_reads {
            assert!(plan.segments.is_empty());
            assert!(plan.connections.iter().all(|edge| {
                edge.fallback == Some(mf_compiler::TypedFallbackReason::ObservedOutput)
            }));
        } else {
            assert_eq!(plan.segments.len(), 1);
            assert_eq!(plan.segments[0].positions, [0, 1, 2, 3]);
            assert!(plan.connections.iter().all(|edge| edge.fallback.is_none()));
        }
    }
}

#[cfg(feature = "codegen")]
#[test]
fn typed_plan_benchmark_reports_both_scenarios() {
    let mut reports = Vec::new();
    benchmark_typed_plan::run(&["4".into(), "2".into()], |report| reports.push(report));
    assert_eq!(reports.len(), 2);
    for (report, control_reads) in reports.iter().zip([false, true]) {
        assert_eq!(report["nodes"], 4);
        assert_eq!(report["iterations"], 2);
        assert_eq!(report["control_reads"], control_reads);
        assert_eq!(report["connections"], 3);
        assert_eq!(report["segments"], usize::from(!control_reads));
        for name in ["prepare_ms", "typed_plan_median_ms"] {
            let elapsed = report[name].as_f64().unwrap();
            assert!(elapsed.is_finite() && elapsed >= 0.0);
        }
    }
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
