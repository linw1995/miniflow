extern crate mfn_core as _;

mod common;

#[path = "fixtures/observation_capture.rs"]
mod capture;

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

fn tool_body(value: &str) -> Value {
    json!({
        "version": "2026-11-01", "dependencies": {},
        "nodes": [{"id":"leaf", "kind":"builtin.parameters", "config":{"parameters":{
            "content":{"type":"string", "secret":false, "default":value}
        }}}],
        "outputs":[{"name":"content", "node":"leaf", "port":"content"}]
    })
}

fn subgraph_router() -> Value {
    json!({
        "version":"2026-11-01", "dependencies":{},
        "flow_arguments":{
            "first":{"definition":tool_body("one"), "parameters":{}},
            "second":{"definition":tool_body("two"), "parameters":{}},
            "unowned":{"definition":tool_body("must not be prepared"), "parameters":{}}
        },
        "nodes":[
            {"id":"params", "kind":"builtin.parameters", "config":{"parameters":{"route":{"type":"string", "secret":false}}}},
            {"id":"arguments", "kind":"builtin.constant", "config":{"value":{}}},
            {"id":"execute", "kind":"builtin.router", "config":{
                "subgraphs":{"alias":{"parameter":"first"}, "other":{"parameter":"second"}},
                "arguments_object":true, "capture_errors":true
            }, "inputs":{
                "route":{"from":{"node":"params", "port":"route"}},
                "arguments":{"from":{"node":"arguments", "port":"value"}}
            }}
        ],
        "outputs":[{"name":"result", "node":"execute", "port":"result"}]
    })
}

#[test]
fn router_owns_only_explicitly_passed_subgraphs_and_observes_branch_aliases() {
    use mf_telemetry::{
        event::{Event, ScopePathEntry},
        identity::RunId,
    };
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = subgraph_router();
    // An unowned lexical parameter must not be prepared or exposed under this router.
    value["flow_arguments"]["unowned"]["definition"]["nodes"][0]["kind"] =
        json!("unlinked.provider");
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    assert_eq!(description.injected_bodies.len(), 2);
    assert_eq!(description.nodes.len(), 3);
    let binding = ScopePathEntry::CallBinding {
        call_id: "execute".into(),
        binding: "alias".into(),
    };
    description
        .validate_scope_path(std::slice::from_ref(&binding))
        .unwrap();
    assert!(description.loop_body(&[binding.body_key()]).is_some());
    assert!(
        description
            .validate_scope_path(&[ScopePathEntry::Call {
                call_id: "execute".into()
            }])
            .is_err()
    );
    assert!(description.loop_body(&["execute".into()]).is_none());
    assert_eq!(
        plan.manifest(&registry).unwrap().description().unwrap(),
        description
    );
    let frozen =
        WorkflowDefinition::from_json(&serde_json::to_string(&plan.definition).unwrap()).unwrap();
    let rebuilt = compile_definition(&frozen, &registry).unwrap();
    assert_eq!(rebuilt.definition, plan.definition);
    let harness = capture::Harness::new(true);
    for (route, expected) in [("alias", "one"), ("other", "two")] {
        let observation = harness
            .observer()
            .start_with_description(description.clone(), RunId::new())
            .unwrap();
        let outputs = mf_compiler::execute_compiled_with_inputs(
            &rebuilt,
            &registry,
            arguments(route),
            Some(observation),
        )
        .unwrap();
        assert_eq!(
            outputs["result"]["outputs"]["content"].as_str(),
            Some(expected)
        );
    }
    let mut branches = std::collections::BTreeSet::new();
    for record in harness.records() {
        let event = record.decode().unwrap();
        event.validate_for(&description).unwrap();
        if let Event::NodeStarted { node, .. } = event.event
            && node.id == "leaf"
        {
            let [ScopePathEntry::CallBinding { call_id, binding }] = node.path.as_slice() else {
                panic!("missing router binding scope")
            };
            assert_eq!(call_id, "execute");
            branches.insert(binding.clone());
        }
    }
    assert_eq!(branches, ["alias".to_owned(), "other".to_owned()].into());
    assert!(!harness.spans.get_finished_spans().unwrap().is_empty());
    let flow = instantiate_compiled(&rebuilt, &registry).unwrap();
    for unauthorized in ["first", "second", "unowned", "missing"] {
        assert!(flow.execute_with_inputs(arguments(unauthorized)).is_err());
    }
}

#[test]
fn subgraph_router_rejects_mixed_malformed_and_missing_parameter_mappings() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for config in [
        json!({"routes":["alias"], "subgraphs":{}, "arguments_object":true, "capture_errors":true}),
        json!({"subgraphs":null, "arguments_object":true, "capture_errors":true}),
        json!({"subgraphs":{"alias":{"parameter":"missing"}}, "arguments_object":true, "capture_errors":true}),
        json!({"subgraphs":{"else":{"parameter":"first"}}, "arguments_object":true, "capture_errors":true}),
        json!({"subgraphs":{" ":{"parameter":"first"}}, "arguments_object":true, "capture_errors":true}),
        json!({"subgraphs":{"alias":{"parameter":" "}}, "arguments_object":true, "capture_errors":true}),
        json!({"subgraphs":{"alias":{"parameter":"first", "definition":tool_body("forged")}}, "arguments_object":true, "capture_errors":true}),
        json!({"subgraphs":{}, "arguments_object":false, "capture_errors":true}),
        json!({"subgraphs":{}, "arguments_object":true, "capture_errors":false}),
    ] {
        let mut value = subgraph_router();
        value["nodes"][2]["config"] = config;
        assert!(compile_definition(&serde_json::from_value(value).unwrap(), &registry).is_err());
    }
    let mut value = subgraph_router();
    value["version"] = json!("2026-10-28");
    value.as_object_mut().unwrap().remove("flow_arguments");
    value["nodes"][2]["config"]["subgraphs"] = json!({});
    let error = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap_err();
    assert!(
        error.to_string().contains("require version 2026-11-01"),
        "{error}"
    );
}

#[test]
fn empty_subgraph_router_prepares_without_executable_authority() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = subgraph_router();
    value["nodes"][2]["config"]["subgraphs"] = json!({});
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    assert!(
        mf_compiler::describe_compiled(&plan)
            .unwrap()
            .injected_bodies
            .is_empty()
    );
    assert!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(arguments("alias"))
            .is_err()
    );
}

#[test]
fn router_checks_every_owned_host_definition_and_looks_up_parameter_not_alias() {
    use mf_runtime::{PortSpec, PreparedSubgraph, ValueRef, ValueType};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let count = Arc::new(AtomicUsize::new(0));
    let mut registry = NodeRegistry::from_inventory().unwrap();
    for parameter in ["first", "second"] {
        let count = count.clone();
        let definition: WorkflowDefinition =
            serde_json::from_value(tool_body(if parameter == "first" { "one" } else { "two" }))
                .unwrap();
        registry.register_flow(parameter, definition, PreparedSubgraph::new(vec![], vec![PortSpec::new("content", ValueType::String, true)], move |context| {
            count.fetch_add(1, Ordering::SeqCst);
            assert!(context.workflow_arguments().0["leaf.content"].is_secret());
            assert_eq!(context.invocation_path(), [mf_telemetry::event::ScopePathEntry::CallBinding {
                call_id:"execute".into(), binding: if parameter == "first" { "alias" } else { "other" }.into()
            }]);
            Ok([("content".into(), context.workflow_arguments().0["leaf.content"].clone())].into())
        }).with_input_schema(serde_json::from_value(json!({"inputs":{"leaf":{"content":{"type":"string", "required":true, "binding":{"secret":true}}}}, "stdin":{}})).unwrap())).unwrap();
    }
    let mut value = subgraph_router();
    value["nodes"][0]["config"]["parameters"]["arguments"] =
        json!({"type":"object", "secret":false});
    value["nodes"][2]["inputs"]["arguments"] =
        json!({"from":{"node":"params", "port":"arguments"}});
    let definition = serde_json::from_value(value.clone()).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    for route in ["alias", "other"] {
        let outputs = flow
            .execute_with_inputs(WorkflowArguments(
                [
                    ("params.route".into(), route.into()),
                    (
                        "params.arguments".into(),
                        ValueRef::object([(
                            "leaf.content".into(),
                            ValueRef::from("secret").into_secret(),
                        )]),
                    ),
                ]
                .into(),
            ))
            .unwrap();
        assert_eq!(
            outputs["result"]["outputs"]["content"].as_str(),
            Some("secret")
        );
        assert!(outputs["result"]["outputs"]["content"].is_secret());
    }
    assert_eq!(count.load(Ordering::SeqCst), 2);
    let error = plan.generate_execution_plans(&registry).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("host Flow executors cannot be emitted"),
        "{error}"
    );
    for parameter in ["first", "second"] {
        let mut drift = value.clone();
        drift["flow_arguments"][parameter]["definition"]["nodes"][0]["config"]["parameters"]["content"]
            ["default"] = json!("drift");
        let error =
            compile_definition(&serde_json::from_value(drift).unwrap(), &registry).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("host Flow binding differs from the frozen call definition"),
            "{error}"
        );
    }
}

#[test]
fn source_free_generated_router_executes_owned_aliased_tool_bodies() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = subgraph_router();
    value["dependencies"] = json!({"core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")}});
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let artifacts = plan.generate_runner_execution_plans(&registry).unwrap();
    assert!(
        artifacts
            .rust_source
            .contains("instantiate_flow_arguments_with_metadata")
    );
    let root = tempfile::tempdir().unwrap();
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
    for route in ["alias", "other", "alias"] {
        let expected = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(arguments(route))
            .unwrap();
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
            json!(expected)
        );
    }
    assert!(
        !Command::new(&output)
            .arg("--params.route=unowned")
            .current_dir(root.path())
            .env("PATH", "")
            .output()
            .unwrap()
            .status
            .success()
    );
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
