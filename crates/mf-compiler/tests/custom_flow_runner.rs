//! The generated runner links a real external provider, not a built-in Flow consumer.
#![cfg(feature = "codegen")]

extern crate mfn_core as _;

mod common;
#[path = "fixtures/flow-arguments-provider/src/lib.rs"]
mod provider;

use mf_compiler::{
    CompileRequest, NodeRegistry, SupportPackages, WorkflowDefinition, compile_definition,
    compile_project, instantiate_compiled,
};
use mf_runtime::{FlowRuntime, WorkflowArguments, WorkflowManifest};
use mf_telemetry::event::ScopePathEntry;
use serde_json::{Value, json};
use std::{fs, process::Command};

fn child(content: &str) -> Value {
    json!({
        "version":"2026-10-28", "dependencies":{},
        // Identical node names must not collapse the two distinct binding bodies.
        "nodes":[{"id":"leaf", "kind":"builtin.constant", "config":{"value":content}}],
        "outputs":[{"name":"content", "node":"leaf", "port":"value"}]
    })
}

#[test]
fn old_root_iteration_keeps_complete_typed_scope_orders_in_a_source_free_runner() {
    let root = tempfile::tempdir().unwrap();
    let provider_dir = root.path().join("provider");
    fs::create_dir_all(provider_dir.join("src")).unwrap();
    fs::write(
        provider_dir.join("Cargo.toml"),
        include_str!("fixtures/flow-arguments-provider/Cargo.toml").replace(
            "\"../../../../mf-runtime\"",
            &serde_json::to_string(&common::crates_dir().join("mf-runtime")).unwrap(),
        ),
    )
    .unwrap();
    fs::write(
        provider_dir.join("src/lib.rs"),
        include_str!("fixtures/flow-arguments-provider/src/lib.rs"),
    )
    .unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-10-03", "dependencies": {
            "provider": {"package": "fixture-flow-arguments-provider", "path": provider_dir},
            "core": {"package": "mfn-core", "path": common::crates_dir().join("builtin-nodes/core")}
        },
        "nodes": [
            {"id": "items", "kind": "builtin.constant", "config": {"value": ["left", "right"]}},
            {"id": "each", "kind": "builtin.iteration", "config": {"body": {"version":"2026-10-03", "dependencies":{},
                "nodes": [{"id": "custom", "kind": provider::KIND, "config": {
                    "left": {"definition": child("one")}, "right": {"definition": child("two")}
                }}],
                "edges": [{"from_node": "%iteration", "from_output": "item", "to_node": "custom", "to_input": "select"}],
                "outputs": [{"name":"result", "node": "custom", "port": "content"}]
            }}}
        ],
        "edges": [{"from_node": "items", "from_output": "value", "to_node": "each", "to_input": "items"}],
        "outputs": [{"name": "results", "node": "each", "port": "results"}]
    })).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    assert_eq!(
        plan.graph.definition.version,
        mf_compiler::WorkflowDefinitionVersion::V2026_10_03
    );
    let each = &plan.graph.children[&"each".into()]["body"];
    let custom = &each.children[&"custom".into()];
    assert_eq!(
        custom.keys().map(String::as_str).collect::<Vec<_>>(),
        ["left", "right"]
    );
    for pointer in ["left", "right"] {
        let owner = each
            .definition
            .nodes
            .iter()
            .find(|node| node.id.as_str() == "custom")
            .unwrap();
        assert_eq!(
            custom[pointer].child_scope_key(owner, pointer),
            ScopePathEntry::binding_key("custom", pointer)
        );
    }
    // Callable children require origin-aware observation even beneath a legacy
    // Iteration. The authored version stays old while all real scopes are retained.
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    assert_eq!(
        description.version,
        mf_telemetry::description::WorkflowDescriptionVersion::V2026_11_09
    );
    assert_eq!(description.loop_bodies.len(), 3);
    assert_eq!(
        description
            .body_paths_for_node(&["each".into()], "custom")
            .len(),
        2
    );
    let expected = json!({"results": ["one", "two"]});
    assert_eq!(
        serde_json::to_value(
            instantiate_compiled(&plan, &registry)
                .unwrap()
                .execute()
                .unwrap()
        )
        .unwrap(),
        expected
    );
    let generated = plan.generate_runner_execution_plans(&registry).unwrap();
    let manifest = WorkflowManifest::from_bytes(&generated.manifest_bytes).unwrap();
    assert_eq!(manifest.graph, plan.graph);
    assert_eq!(manifest.description().unwrap(), description);
    let source = root.path().join("flow.json");
    fs::write(&source, serde_json::to_vec(&definition).unwrap()).unwrap();
    let build = root.path().join("build");
    let output = root.path().join("runner");
    compile_project(&CompileRequest {
        definition: &source,
        output: &output,
        locked: false,
        build_dir: Some(&build),
        support: &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    })
    .unwrap();
    fs::remove_file(&source).unwrap();
    fs::remove_dir_all(&build).unwrap();
    fs::remove_dir_all(&provider_dir).unwrap();
    let result = Command::new(output)
        .current_dir(root.path())
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        expected
    );
}

#[test]
fn source_free_generated_runner_executes_external_provider_flow_bindings() {
    let root = tempfile::tempdir().unwrap();
    let provider_dir = root.path().join("provider");
    fs::create_dir_all(provider_dir.join("src")).unwrap();
    let manifest = include_str!("fixtures/flow-arguments-provider/Cargo.toml").replace(
        "\"../../../../mf-runtime\"",
        &serde_json::to_string(&common::crates_dir().join("mf-runtime")).unwrap(),
    );
    fs::write(provider_dir.join("Cargo.toml"), manifest).unwrap();
    fs::write(
        provider_dir.join("src/lib.rs"),
        include_str!("fixtures/flow-arguments-provider/src/lib.rs"),
    )
    .unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-28",
        "dependencies":{
            "provider":{"package":"fixture-flow-arguments-provider", "path":provider_dir},
            "core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")}
        },
        "nodes":[
            {"id":"params", "kind":"builtin.parameters", "config":{
                "parameters":{"select":{"type":"string", "secret":false}}
            }},
            {"id":"custom", "kind":provider::KIND, "config":{
                "left":{"definition":child("one")}, "right":{"definition":child("two")}
            }, "inputs":{"select":{"from":{"node":"params", "port":"select"}}}}
        ],
        "outputs":[{"name":"content", "node":"custom", "port":"content"}]
    }))
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    // Exercise the original provider as well as its copied, generated-runner version.
    // The interpreted and generated paths must enforce the same local alias authority.
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    for (selection, expected) in [("left", "one"), ("right", "two"), ("left", "one")] {
        let arguments = WorkflowArguments::try_from(json!({"params.select":selection})).unwrap();
        let outputs = FlowRuntime::default()
            .execute_with_inputs(&flow, arguments)
            .unwrap();
        assert_eq!(
            serde_json::to_value(outputs).unwrap(),
            json!({"content":expected})
        );
    }
    let error = FlowRuntime::default()
        .execute_with_inputs(
            &flow,
            WorkflowArguments::try_from(json!({"params.select":"first"})).unwrap(),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown local Flow binding `first`"),
        "{error}"
    );
    let generated = plan.generate_runner_execution_plans(&registry).unwrap();
    let manifest = WorkflowManifest::from_bytes(&generated.manifest_bytes).unwrap();
    assert_eq!(
        manifest.node_definition("custom").unwrap().kind,
        provider::KIND
    );
    let description = manifest.description().unwrap();
    assert_eq!(description.loop_bodies.len(), 2);
    for binding in ["left", "right"] {
        let scope = ScopePathEntry::Call {
            call_id: ScopePathEntry::binding_key("custom", binding),
        };
        description
            .validate_scope_path(std::slice::from_ref(&scope))
            .unwrap();
        let body = description
            .loop_body(&[scope.node_id().to_owned()])
            .unwrap();
        assert_eq!(body.nodes.len(), 1);
        assert_eq!(body.nodes[0].id, "leaf");
    }

    let source = root.path().join("flow.json");
    fs::write(&source, serde_json::to_vec(&definition).unwrap()).unwrap();
    let output = root.path().join("runner");
    let build = root.path().join("build");
    compile_project(&CompileRequest {
        definition: &source,
        output: &output,
        locked: false,
        build_dir: Some(&build),
        support: &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    })
    .unwrap();
    fs::remove_file(&source).unwrap();
    fs::remove_dir_all(&build).unwrap();
    fs::remove_dir_all(&provider_dir).unwrap();

    for (selection, expected) in [("left", "one"), ("right", "two")] {
        let result = Command::new(&output)
            .arg(format!("--params.select={selection}"))
            .current_dir(root.path())
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap(),
            json!({"content":expected})
        );
    }
    // Names without a configured local field are not executable authority.
    let rejected = Command::new(&output)
        .arg("--params.select=first")
        .current_dir(root.path())
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(!rejected.status.success());
}
