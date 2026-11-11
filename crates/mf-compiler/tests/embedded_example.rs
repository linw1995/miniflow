//! Exercise the embedded example's ordinary Flow-valued edges, not a copied fixture.
extern crate mfn_core as _;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_resolved_definition, instantiate_compiled,
};
use mf_runtime::WorkflowArguments;
use mf_telemetry::event::{FlowOrigin, ScopePathEntry};
use serde_json::json;
use std::path::Path;

fn example() -> WorkflowDefinition {
    // These dependencies belong to the embedding host, not a generated runner project.
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/embedded-workflows");
    let workflow = mf_compiler::FlowSourceBundle {
        entry: "answer.json".into(),
        sources: ["answer.json", "answer-apply.json", "answer-body.json"]
            .into_iter()
            .map(|name| {
                (
                    name.into(),
                    std::fs::read_to_string(directory.join(name)).unwrap(),
                )
            })
            .collect(),
    };
    let registry = NodeRegistry::from_inventory().unwrap();
    workflow.resolve(&registry).unwrap().definition
}

#[test]
fn real_example_passes_a_flow_over_an_edge_and_executes_default_and_override() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition = example();
    assert_eq!(
        definition.dependencies["core"].path.as_deref(),
        Some(Path::new("../../crates/builtin-nodes/core"))
    );
    let plan = compile_resolved_definition(&definition, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    assert_eq!(flow.execute().unwrap()["answer"].as_i64(), Some(42));
    let arguments = WorkflowArguments::try_from(json!({"params.value":43})).unwrap();
    assert_eq!(
        flow.execute_with_inputs(arguments).unwrap()["answer"].as_i64(),
        Some(43)
    );

    let graph = mf_compiler::describe_compiled(&plan).unwrap();
    let manifest = plan.manifest(&registry).unwrap();
    assert_eq!(manifest.description().unwrap(), graph);
    let call = manifest.node_definition("callA").unwrap();
    let children = call.flow_parameters(&["body"]).unwrap();
    assert_eq!(children["body"].nodes[0].id.as_str(), "params");
    assert_eq!(children["body"].nodes[1].kind, "workflow.call");
    let paths = graph.body_paths_for_node(&[], "policy");
    assert_eq!(paths.len(), 1);
    let scopes = [
        ScopePathEntry::Call {
            call_id: "callA".into(),
        },
        ScopePathEntry::Flow {
            call_id: "apply".into(),
            definition: FlowOrigin {
                workflow_id: graph.workflow_id.clone(),
                path: paths[0].clone(),
            },
        },
    ];
    graph.validate_scope_path(&scopes).unwrap();
    assert!(
        graph
            .body_paths_for_node(&["callA".into()], "apply")
            .is_empty()
    );
    assert_eq!(graph.body(paths[0]).unwrap().nodes[0].id, "echo");
}
