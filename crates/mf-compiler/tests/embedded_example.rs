//! Exercise the embedded example's actual provider in memory, not a copied fixture.
extern crate mfn_core as _;

use embedded_flow_provider as example_nodes;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_resolved_definition, instantiate_compiled,
};
use mf_runtime::{NodeFactory, WorkflowArguments};
use mf_telemetry::event::ScopePathEntry;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

fn example() -> WorkflowDefinition {
    // These dependencies belong to the embedding host, not a generated runner project.
    mf_compiler::load_embedded_workflow(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/embedded-workflows/answer.json"),
    )
    .unwrap()
    .definition
}

#[test]
fn real_example_executes_default_and_override_and_preserves_binding_description() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition = example();
    assert_eq!(
        definition.dependencies["host"].path.as_deref(),
        Some(Path::new("./provider"))
    );
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
    let call = manifest.node_definition("callA").unwrap();
    assert_eq!(call.kind, "workflow.call");
    let children = call.flow_parameters(&["/body"]).unwrap();
    assert_eq!(
        children.keys().map(String::as_str).collect::<Vec<_>>(),
        ["/body"]
    );
    assert_eq!(children["/body"].nodes[0].id.as_str(), "apply");
    assert_eq!(children["/body"].nodes[0].kind, example_nodes::KIND);
    assert_eq!(manifest.description().unwrap(), graph);
    let scope = ScopePathEntry::Call {
        call_id: ScopePathEntry::binding_key("apply", "/policy/body"),
    };
    let scopes = [
        ScopePathEntry::Call {
            call_id: "callA".into(),
        },
        scope.clone(),
    ];
    graph.validate_scope_path(&scopes).unwrap();
    let call_path = vec!["callA".into()];
    let path = vec!["callA".into(), scope.node_id().to_owned()];
    assert_eq!(graph.body_paths_for_node(&[], "callA"), vec![&call_path]);
    assert_eq!(graph.body_paths_for_node(&call_path, "apply"), vec![&path]);
    assert_eq!(graph.loop_bodies.len(), 2);
    let apply = graph.body(&call_path).unwrap();
    assert_eq!(apply.nodes.len(), 1);
    assert_eq!(apply.nodes[0].id, "apply");
    let body = graph.body(&path).unwrap();
    assert_eq!(body.nodes.len(), 1);
    assert_eq!(body.nodes[0].id, "echo");
}

#[test]
fn example_factory_requires_prepared_body_and_exact_alias() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let registration = registry.get(example_nodes::KIND).unwrap();
    assert!(matches!(registration.factory, NodeFactory::Subgraph { .. }));
    let error = registration.instantiate(json!({})).err().unwrap();
    assert!(
        error
            .to_string()
            .contains("requires a prepared `/policy/body` binding")
    );
    for (config, expected) in [
        (json!({}), "missing prepared `/policy/body` binding"),
        (
            json!({"policy":{"body":{
                "version":"2026-10-28", "dependencies":{}, "nodes":[]
            }}}),
            "receiver-selected configuration fields",
        ),
    ] {
        let error = registration
            .instantiate_subgraphs("apply", config, BTreeMap::new())
            .err()
            .unwrap();
        assert!(error.to_string().contains(expected), "{error}");
    }
}

fn rejects_body(body: Value, expected: &str) {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = serde_json::to_value(example()).unwrap();
    value["nodes"][1]["config"]["body"]["nodes"][0]["config"]["policy"]["body"] = body;
    let definition = serde_json::from_value(value).unwrap();
    let error = compile_resolved_definition(&definition, &registry).unwrap_err();
    assert!(error.to_string().contains(expected), "{error}");
}

#[test]
fn example_rejects_missing_or_multiple_child_startup_inputs() {
    let original = serde_json::to_value(example()).unwrap();
    let mut body =
        original["nodes"][1]["config"]["body"]["nodes"][0]["config"]["policy"]["body"].clone();
    body["nodes"][0] = json!({"id":"echo", "kind":"builtin.constant", "config":{"value":42}});
    rejects_body(body, "the body must have one required startup input");

    let mut body =
        original["nodes"][1]["config"]["body"]["nodes"][0]["config"]["policy"]["body"].clone();
    body["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"extra", "kind":"builtin.identity"}));
    rejects_body(
        body,
        "one explicit required input and one terminal `answer` output",
    );
}

#[test]
fn example_rejects_incompatible_outputs_and_fallback_backed_startup_input() {
    let original = serde_json::to_value(example()).unwrap();
    let body = &original["nodes"][1]["config"]["body"]["nodes"][0]["config"]["policy"]["body"];
    for outputs in [
        json!([{"name":"wrong", "node":"echo", "port":"value"}]),
        json!([{"name":"answer", "node":"echo", "port":"value", "optional":true}]),
        json!([
            {"name":"answer", "node":"echo", "port":"value"},
            {"name":"extra", "node":"echo", "port":"value"}
        ]),
    ] {
        let mut changed = body.clone();
        changed["outputs"] = outputs;
        rejects_body(
            changed,
            "one explicit required input and one terminal `answer` output",
        );
    }
    let mut changed = body.clone();
    changed["nodes"][0] = json!({
        "id":"echo", "kind":"builtin.parameters",
        "config":{"parameters":{"value":{"type":"int", "default":42, "secret":false}}}
    });
    rejects_body(
        changed,
        "one explicit required input and one terminal `answer` output",
    );
}
