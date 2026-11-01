extern crate mfn_core as _;

mod common;

use mf_compiler::{
    CompileRequest, NodeRegistry, SupportPackages, WorkflowDefinition, compile_definition,
    compile_project, instantiate_compiled,
};
use mf_runtime::WorkflowArguments;
use serde_json::{Value, json};
use std::{fs, process::Command};

fn definition() -> WorkflowDefinition {
    let mut value: Value =
        serde_json::from_str(include_str!("../../../examples/router-flow.json")).unwrap();
    value["dependencies"]["core"]["path"] = json!(common::crates_dir().join("builtin-nodes/core"));
    for port in ["first", "second", "else"] {
        value["outputs"].as_array_mut().unwrap().push(json!({
            "name": port, "node": "route", "port": port, "optional": true
        }));
    }
    serde_json::from_value(value).unwrap()
}

fn arguments(route: &str) -> WorkflowArguments {
    WorkflowArguments::try_from(json!({"params.route": route})).unwrap()
}

fn expected(route: &str) -> Value {
    let (gate, result) = match route {
        "first" => ("first", "First branch"),
        "second" => ("second", "Second branch"),
        _ => ("else", "Unknown route"),
    };
    json!({"result": result, gate: true})
}

#[test]
fn user_authored_router_remains_an_ordinary_runtime_node() {
    let registry = NodeRegistry::from_inventory().unwrap();
    assert_eq!(mfn_core::ROUTER_KIND, "builtin.router");
    let definition = definition();
    let plan = compile_definition(&definition, &registry).unwrap();
    assert_eq!(plan.definition.nodes.len(), definition.nodes.len());
    let router = plan
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "route")
        .unwrap();
    assert_eq!(router.kind, "builtin.router");
    assert_eq!(router.config, json!({"routes": ["first", "second"]}));

    let flow = instantiate_compiled(&plan, &registry).unwrap();
    for route in ["first", "second", "missing", "", "first"] {
        let outputs = flow.execute_with_inputs(arguments(route)).unwrap();
        assert_eq!(json!(outputs), expected(route));
    }
}

#[test]
fn unselected_routes_do_not_execute_downstream_tasks() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (route, selected) in [
        ("first", "first"),
        ("second", "second"),
        ("missing", "else"),
    ] {
        let mut value = serde_json::to_value(definition()).unwrap();
        for port in ["first", "second", "else"] {
            if port == selected {
                continue;
            }
            let id = format!("tripwire_{port}");
            value["nodes"].as_array_mut().unwrap().push(json!({
                "id": id, "kind": "builtin.assert", "config": {
                    "condition": {"source": {"output": "params.route", "path": ""}, "operator": "eq", "value": "never-selected"},
                    "message": "unselected router branch executed"
                }
            }));
            value["edges"].as_array_mut().unwrap().push(json!({
                "from_node": "route", "from_output": port, "to_node": id
            }));
        }
        let definition = serde_json::from_value(value).unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        assert_eq!(
            json!(flow.execute_with_inputs(arguments(route)).unwrap()),
            expected(route)
        );
        // The same tripwire must fail if its route is selected instead of skipped.
        let other = if selected == "first" {
            "second"
        } else {
            "first"
        };
        assert!(flow.execute_with_inputs(arguments(other)).is_err());
    }
}

#[test]
fn standalone_runner_uses_only_core_and_matches_runtime_route_selection() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("flow.json");
    fs::write(&source, serde_json::to_vec(&definition()).unwrap()).unwrap();
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

    for route in ["first", "second", "missing", "first"] {
        let result = Command::new(&output)
            .arg(format!("--params.route={route}"))
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
            expected(route)
        );
    }
}
