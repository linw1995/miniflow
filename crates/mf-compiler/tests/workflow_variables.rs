mod common;
#[path = "fixtures/multi-nodes/src/task_stream.rs"]
mod fixture;
extern crate mfn_code as _;
extern crate mfn_core as _;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, describe_compiled, instantiate_compiled,
    plan_definition,
};
use serde_json::{Value, json};
use std::{
    error::Error,
    sync::{Arc, Mutex},
};

fn definition() -> Value {
    json!({
        "version": "2026-11-13",
        "variables": [
            {"name": "count", "type": "int", "initial": 0},
            {"name": "maybe", "type": {"nullable": "string"}, "initial": null}
        ],
        "nodes": [
            {"id": "params", "kind": "builtin.parameters", "config": {"parameters": {
                "value": {"type": "int", "default": 7, "secret": false}
            }}},
            {"id": "write", "kind": "workflow.assign", "config": {"variable": "count"},
             "inputs": {"value": {"from": {"node": "params", "port": "value"}}}},
            {"id": "read", "kind": "builtin.identity", "inputs": {
                "input": {"from": {"node": "%root", "port": "count"}}
            }}
        ],
        "edges": [{"from_node": "write", "from_output": "done", "to_node": "read"}],
        "outputs": [
            {"name": "count", "node": "%root", "port": "count"},
            {"name": "observed", "node": "read", "port": "value"},
            {"name": "maybe", "node": "%root", "port": "maybe"}
        ]
    })
}

fn parse(value: Value) -> WorkflowDefinition {
    serde_json::from_value(value).unwrap()
}

fn run(value: Value) -> Value {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(value), &registry).unwrap();
    serde_json::to_value(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap(),
    )
    .unwrap()
}

fn compile_error(value: Value) -> mf_compiler::WorkflowCompileError {
    compile_definition(&parse(value), &NodeRegistry::from_inventory().unwrap()).unwrap_err()
}

fn find_source<'a, T: Error + 'static>(error: &'a (dyn Error + 'static)) -> Option<&'a T> {
    let mut source = Some(error);
    while let Some(error) = source {
        if let Some(typed) = error.downcast_ref() {
            return Some(typed);
        }
        source = error.source();
    }
    None
}

#[test]
fn assignments_refresh_root_outputs_and_preserve_startup_inputs_and_fresh_invocations() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(definition()), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    #[cfg(feature = "codegen")]
    {
        let typed = mf_compiler::plan_typed_segments(&flow, true, true);
        assert!(typed.segments.is_empty());
        assert!(
            typed
                .connections
                .iter()
                .all(|connection| connection.fallback
                    == Some(mf_compiler::TypedFallbackReason::MutableState))
        );
    }
    assert_eq!(
        flow.input_schema().inputs["params"]
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["value"]
    );
    assert!(
        !flow
            .input_schema()
            .inputs
            .contains_key(mf_runtime::ROOT_SOURCE_ID)
    );
    let mut context = mf_runtime::ExecutionContext::default();
    for value in [42, 9, 7] {
        context.set_workflow_arguments(
            mf_runtime::WorkflowArguments::try_from(json!({"params.value": value})).unwrap(),
        );
        let output = flow.execute_in_context(&mut context).unwrap();
        assert_eq!(
            serde_json::to_value(output).unwrap(),
            json!({"count": value, "observed": value, "maybe": null})
        );
        assert!(context.invocation_path().is_empty());
    }
    let snapshots = Arc::new(Mutex::new(Vec::new()));
    let captured = snapshots.clone();
    let mut context = mf_runtime::ExecutionContext::default();
    context.set_snapshot_recorder(mf_runtime::SnapshotRecorder::new(move |event| {
        captured.lock().unwrap().push(event.scope.clone());
    }));
    assert_eq!(
        serde_json::to_value(flow.execute_in_context(&mut context).unwrap()).unwrap(),
        json!({"count": 7, "observed": 7, "maybe": null})
    );
    assert!(snapshots.lock().unwrap().iter().all(Vec::is_empty));
    let description = describe_compiled(&plan).unwrap();
    assert!(description.loop_bodies.is_empty());
    let (nodes, _) =
        mf_compiler::prepare_compiled_graph_with_layouts(&plan.graph, &registry, None).unwrap();
    let targets = plan.node_targets(&nodes).unwrap();
    assert_eq!(
        targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>(),
        ["params", "read"]
    );
}

fn gate_writes(mut value: Value) -> Value {
    value["nodes"].as_array_mut().unwrap().push(json!({
        "id": "route", "kind": "builtin.if_else", "config": {"branches": [{"id": "update", "condition": {
            "source": {"output": "params.value", "path": ""}, "operator": "ne", "value": -1
        }}]}
    }));
    value["edges"].as_array_mut().unwrap().extend([
        json!({"from_node": "params", "to_node": "route"}),
        json!({"from_node": "route", "from_output": "update", "to_node": "write"}),
    ]);
    value["outputs"][1]["optional"] = json!(true);
    value
}

#[test]
fn skipped_writes_keep_initial_values_and_skip_dependent_readers() {
    let mut value = gate_writes(definition());
    value["nodes"][0]["config"]["parameters"]["value"]["default"] = json!(-1);
    assert_eq!(run(value), json!({"count": 0, "maybe": null}));
}

#[test]
fn stateful_domains_keep_prior_reads_and_multiple_updates_ordered() {
    let mut value = definition();
    value["nodes"].as_array_mut().unwrap().extend([
        json!({"id":"before", "kind":"builtin.identity", "inputs":{"input":{"from":{"node":"%root", "port":"count"}}}}),
        json!({"id":"second", "kind":"workflow.assign", "config":{"variable":"count"}, "inputs":{"value":{"from":{"node":"before", "port":"value"}}}})
    ]);
    value["edges"].as_array_mut().unwrap().extend([
        json!({"from_node":"before", "from_output":"value", "to_node":"write"}),
        json!({"from_node":"write", "from_output":"done", "to_node":"second"}),
        json!({"from_node":"second", "from_output":"done", "to_node":"read"}),
    ]);
    value["outputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"before", "node":"before", "port":"value"}));
    assert_eq!(
        run(value),
        json!({"count":0,"observed":0,"maybe":null,"before":0})
    );
}

#[test]
fn root_variables_reject_explicit_and_provider_selected_streaming() {
    let mut value = definition();
    value["execution"] = json!({});
    assert!(
        compile_error(value)
            .to_string()
            .contains("finite callable Flow")
    );
    let mut value = definition();
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"lines", "kind":"builtin.readline"}));
    value["outputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"line", "node":"lines", "port":"line"}));
    let error = compile_error(value);
    assert!(
        error.to_string().contains("finite callable Flow"),
        "{error}"
    );
    // A valid finite artifact must also reject a consumer requesting streaming later.
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(definition()), &registry).unwrap();
    let error = mf_compiler::instantiate_stream(&plan, &registry)
        .expect_err("root state must not become an autonomous stream");
    assert!(matches!(
        find_source::<mf_compiler::StreamBuildError>(&error),
        Some(mf_compiler::StreamBuildError::InvalidPlan { message })
            if message.contains("finite callable Flow")
    ));
}

#[test]
fn synthetic_root_source_is_injected_before_ordinary_nodes() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(definition()), &registry).unwrap();
    assert_eq!(
        plan.graph.execution_order[0].as_str(),
        mf_runtime::ROOT_SOURCE_ID
    );
    let source = plan
        .graph
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == mf_runtime::ROOT_SOURCE_ID)
        .expect("the root source is synthesized from the definition field");
    assert_eq!(source.kind, mf_runtime::ROOT_SOURCE_ID);
    assert_eq!(source.config, json!({}));
    // Visiting the source after the assignment must not republish initial literals.
    assert_eq!(
        run(definition()),
        json!({"count": 7, "observed": 7, "maybe": null})
    );
}

#[test]
fn declared_types_not_initial_literals_drive_inference() {
    let mut value = definition();
    value["variables"][0]["type"] = json!("any");
    value["nodes"][0]["config"]["parameters"]["value"] =
        json!({"type": "string", "default": "changed", "secret": false});
    assert_eq!(
        run(value),
        json!({"count": "changed", "observed": "changed", "maybe": null})
    );
}

#[test]
fn invalid_declarations_and_scope_ownership_fail_before_execution() {
    type InvalidEdit = (Box<dyn Fn(&mut Value)>, &'static str);
    let cases: Vec<InvalidEdit> = vec![
        (
            Box::new(|v| v["version"] = json!("2026-11-11")),
            "require definition version 2026-11-13",
        ),
        (
            Box::new(|v| v["variables"] = json!([])),
            "variables must not be empty",
        ),
        (
            Box::new(|v| v["variables"][1]["name"] = json!("count")),
            "duplicate variable",
        ),
        (
            Box::new(|v| v["variables"][0]["name"] = json!(" ")),
            "invalid variable name",
        ),
        (
            Box::new(|v| v["nodes"][1]["config"]["variable"] = json!("missing")),
            "unknown variable `missing`",
        ),
        (
            Box::new(|v| v["nodes"][1]["kind"] = json!("workflow.loop_assign")),
            "assignment requires a state scope",
        ),
        (
            Box::new(|v| {
                v["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"id":"exit", "kind":"workflow.exit_loop"}));
            }),
            "exit requires a state scope",
        ),
        (
            Box::new(|v| {
                v["edges"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"from_node":"params", "from_output":"value", "to_node":"%root"}))
            }),
            "source cannot have incoming dependencies",
        ),
        (
            Box::new(|v| {
                v["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"id":"named", "kind":"workflow.variables", "config":{}}));
            }),
            "unknown kind `workflow.variables`",
        ),
    ];
    for (edit, expected) in cases {
        let mut value = definition();
        edit(&mut value);
        let error = compile_error(value);
        assert!(
            error.to_string().contains(expected),
            "expected {expected}: {error}"
        );
    }
    // Generic assignments have their own version gate even without a declaration.
    let error = compile_error(json!({
        "version":"2026-11-11", "nodes":[
            {"id":"write", "kind":"workflow.assign", "config":{"variable":"count"}}
        ]
    }));
    assert!(
        error
            .to_string()
            .contains("require definition version 2026-11-13"),
        "{error}"
    );
    let mut value = definition();
    value["variables"][0]
        .as_object_mut()
        .unwrap()
        .remove("initial");
    assert!(serde_json::from_value::<WorkflowDefinition>(value).is_err());
    let mut value = definition();
    value["variables"][0]["type"] = json!("invalid");
    assert!(find_source::<mf_runtime::TypeDescriptorError>(&compile_error(value)).is_some());
    let mut value = definition();
    value["variables"][0]["initial"] = json!("wrong");
    assert!(find_source::<mf_runtime::TypeMismatch>(&compile_error(value)).is_some());
}

fn nested() -> Value {
    let mut value = definition();
    value["nodes"].as_array_mut().unwrap().extend([
        json!({"id": "child", "kind": "workflow.call", "config": {"flow": "local-child", "body": definition()}}),
        json!({"id": "repeat", "kind": "workflow.loop", "config": {
            "variables": [{"name": "count", "type": "int", "initial": 10}], "max_iterations": 2,
            "body": {"nodes": [
                {"id":"inc", "kind":"builtin.code", "config": {"language":"cel", "inputs":{"count":"int"}, "code":{"next":"count + 1"}},
                 "inputs":{"count":{"from":{"node":"%loop", "port":"count"}}}},
                {"id":"save", "kind":"workflow.assign", "config":{"variable":"count"}, "inputs":{"value":{"from":{"node":"inc", "port":"next"}}}}
            ]}
        }})
    ]);
    value["nodes"][3]["config"]["body"]["nodes"][0]["config"]["parameters"]["value"]["default"] =
        json!(99);
    value["outputs"].as_array_mut().unwrap().extend([
        json!({"name":"child", "node":"child", "port":"count"}),
        json!({"name":"loop", "node":"repeat", "port":"count"}),
    ]);
    value
}

fn loop_output_updates() -> Value {
    let mut value = nested();
    let repeat = &mut value["nodes"][4]["config"];
    repeat["variables"].as_array_mut().unwrap().push(json!({
        "name":"observed", "type":"int", "initial":0
    }));
    let body = &mut repeat["body"];
    body["nodes"][1]["config"]["variable"] = json!("observed");
    body["nodes"].as_array_mut().unwrap().push(json!({
        "id":"read_observed", "kind":"builtin.identity", "inputs":{
            "input":{"from":{"node":"%loop", "port":"observed"}}
        }
    }));
    body["edges"] = json!([{"from_node":"save", "from_output":"done", "to_node":"read_observed"}]);
    body["outputs"] = json!([{"name":"count", "node":"read_observed", "port":"value"}]);
    value["outputs"].as_array_mut().unwrap().push(json!({
        "name":"loop_observed", "node":"repeat", "port":"observed"
    }));
    value
}

fn exited_loop_output_updates() -> Value {
    let mut value = loop_output_updates();
    let body = &mut value["nodes"][4]["config"]["body"];
    body["nodes"].as_array_mut().unwrap().extend([
        json!({"id":"exit", "kind":"workflow.exit_loop"}),
        json!({"id":"zz_unvisited", "kind":"builtin.code", "config":{
            "language":"cel", "inputs":{"count":"int"}, "code":{"next":"count + 1000"}
        }, "inputs":{"count":{"from":{"node":"%loop", "port":"count"}}}}),
    ]);
    body["edges"].as_array_mut().unwrap().extend([
        json!({"from_node":"save", "from_output":"done", "to_node":"exit"}),
        json!({"from_node":"save", "from_output":"done", "to_node":"zz_unvisited"}),
    ]);
    body["outputs"] = json!([{"name":"count", "node":"zz_unvisited", "port":"next"}]);
    value
}

#[test]
fn generic_assignments_cannot_overlap_end_of_pass_updates() {
    let mut value = nested();
    value["nodes"][4]["config"]["body"]["outputs"] = json!([
        {"name":"count", "node":"inc", "port":"next"}
    ]);
    let error = compile_error(value);
    assert!(matches!(find_source::<mfn_core::LoopBuildError>(&error),
        Some(mfn_core::LoopBuildError::ConflictingBodyOutput { variable }) if variable == "count"));
    assert!(
        error.to_string().contains("immediate assignment"),
        "{error}"
    );
}

#[test]
fn immediate_and_end_of_pass_updates_coexist_without_mutating_root_state() {
    assert_eq!(
        run(loop_output_updates()),
        json!({
            "count":7, "observed":7, "maybe":null, "child":99, "loop":12, "loop_observed":12
        })
    );
}

#[test]
fn explicit_exits_preserve_generic_writes_and_abandon_end_of_pass_selections() {
    assert_eq!(
        run(exited_loop_output_updates()),
        json!({
            "count":7, "observed":7, "maybe":null, "child":99, "loop":10, "loop_observed":11
        })
    );
}

#[test]
fn callable_and_loop_state_remain_isolated_from_parent_root_state() {
    assert_eq!(
        run(nested()),
        json!({"count":7, "observed":7, "maybe":null, "child":99, "loop":12})
    );
    let mut value = nested();
    value["nodes"][4]["config"]["variables"][0]["name"] = json!("local");
    assert!(
        compile_error(value)
            .to_string()
            .contains("unknown variable `count`")
    );
    let mut value = nested();
    value["nodes"][4]["config"]["body"]["variables"] = json!([
        {"name":"nested", "type":"int", "initial":1}
    ]);
    assert!(
        compile_error(value)
            .to_string()
            .contains("container bodies cannot declare them")
    );
}

#[test]
fn compiled_relations_are_authoritative_and_reject_corrupt_state() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(definition()), &registry).unwrap();
    let encoded = plan.to_json().unwrap();
    let restored = mf_compiler::CompiledWorkflow::from_json(&encoded).unwrap();
    assert_eq!(plan, restored);
    for mutation in 0..7 {
        let mut changed = restored.clone();
        let root = changed.graph.relations.root.as_mut().unwrap();
        match mutation {
            0 => root.input = "missing".into(),
            1 => {
                root.writes.insert(root.input.clone(), "count".into());
            }
            2 => {
                root.writes.insert("write".into(), "missing".into());
            }
            3 => {
                root.initial.insert("count".into(), json!("wrong"));
            }
            4 => {
                root.initial.remove("count");
            }
            5 => {
                changed.graph.definition.version =
                    mf_runtime::WorkflowDefinitionVersion::V2026_11_11;
            }
            _ => {
                changed.graph.definition.variables = None;
            }
        }
        assert!(
            instantiate_compiled(&changed, &registry).is_err(),
            "mutation {mutation}"
        );
        assert!(
            mf_runtime::describe_graph(&changed.graph).is_err(),
            "mutation {mutation}"
        );
    }
    for dependency in ["data", "control", "success"] {
        let mut changed = restored.clone();
        let definition = &mut changed.graph.definition;
        match dependency {
            "data" => definition.edges.push(mf_runtime::EdgeDefinition {
                exclusive: false,
                from_node: "params".into(),
                from_output: "value".into(),
                to_node: "%root".into(),
                to_input: "unused".into(),
            }),
            "control" => definition
                .control_edges
                .push(mf_runtime::ControlEdgeDefinition {
                    from_node: "params".into(),
                    from_output: "value".into(),
                    to_node: "%root".into(),
                }),
            _ => definition
                .order_edges
                .push(mf_runtime::OrderEdgeDefinition {
                    from_node: "params".into(),
                    to_node: "%root".into(),
                }),
        }
        changed.graph.execution_order = ["%root", "params", "write", "read"]
            .into_iter()
            .map(mf_runtime::DefinitionId::from)
            .collect();
        let error = mf_runtime::describe_graph(&changed.graph).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("source cannot have incoming dependencies"),
            "{dependency}: {error}"
        );
        assert!(
            instantiate_compiled(&changed, &registry).is_err(),
            "{dependency}"
        );
    }
    // Assignment instructions remain relation-driven; the synthetic source keeps its identity.
    let mut renamed = restored.clone();
    for node in &mut renamed.graph.definition.nodes {
        if node.id.as_str() != mf_runtime::ROOT_SOURCE_ID
            && renamed.graph.relations.instruction(&node.id).is_some()
        {
            node.kind = "unregistered.provider".into();
            node.config = json!({"opaque":true});
        }
    }
    let outputs = instantiate_compiled(&renamed, &registry)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(outputs["count"], json!(7));
    // The canonical synthetic source is not interchangeable with an authored node.
    let mut forged = restored;
    for node in &mut forged.graph.definition.nodes {
        if node.id.as_str() == mf_runtime::ROOT_SOURCE_ID {
            node.kind = "unregistered.provider".into();
        }
    }
    assert!(instantiate_compiled(&forged, &registry).is_err());
    assert!(mf_runtime::describe_graph(&forged.graph).is_err());
}

#[test]
fn finite_stateful_calls_can_forward_task_streams_without_promoting_root_state() {
    let child = json!({"version":"2026-11-13",
        "variables":[{"name":"count","type":"usize","initial":2}],
        "nodes":[
        {"id":"step", "kind":"fixture.task_stream", "inputs":{"count":{"from":{"node":"%root","port":"count"}}}},
        {"id":"save", "kind":"workflow.assign", "config":{"variable":"count"}, "inputs":{"value":{"from":{"node":"step","port":"total"}}}}
    ], "outputs":[{"name":"delta","node":"step","port":"delta"},{"name":"count","node":"%root","port":"count"}]});
    assert!(
        compile_error(child.clone())
            .to_string()
            .contains("finite callable Flow")
    );
    let mut parent = json!({"version":"2026-11-13", "nodes":[
        {"id":"invoke", "kind":"workflow.call", "config":{"flow":"child","body":child}}
    ], "outputs":[{"name":"delta","node":"invoke","port":"delta"},{"name":"count","node":"invoke","port":"count"}]});
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(parent.clone()), &registry).unwrap();
    let stream = mf_compiler::instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(Default::default())
        .unwrap();
    let mut outputs = Vec::new();
    while let Some(output) = stream.recv().unwrap() {
        outputs.push(serde_json::to_value(output.outputs).unwrap());
    }
    stream.join().unwrap();
    assert_eq!(outputs.len(), 3);
    // Terminal and incremental ports belong to independent message domains.
    assert_eq!(
        outputs
            .iter()
            .filter_map(|value| value.get("delta"))
            .collect::<Vec<_>>(),
        [&json!(0), &json!(1)]
    );
    assert_eq!(
        outputs
            .iter()
            .filter_map(|value| value.get("count"))
            .collect::<Vec<_>>(),
        [&json!(2)]
    );
    parent["outputs"].as_array_mut().unwrap().remove(0);
    assert_eq!(run(parent), json!({"count":2}));
}

#[test]
fn state_cannot_be_used_without_an_owning_declaration() {
    let mut value = definition();
    value.as_object_mut().unwrap().remove("variables");
    let error = plan_definition(&parse(value)).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("assignment requires a state scope"),
        "{error}"
    );
}

#[cfg(feature = "codegen")]
#[test]
fn generated_root_constructors_follow_compiled_relations_not_authored_config() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut plan = compile_definition(&parse(definition()), &registry).unwrap();
    for node in &mut plan.graph.definition.nodes {
        if node.id.as_str() != mf_runtime::ROOT_SOURCE_ID
            && plan.graph.relations.instruction(&node.id).is_some()
        {
            node.kind = "unregistered.provider".into();
            node.config = json!({"opaque":true});
        }
    }
    let artifacts = plan.generate_artifacts().unwrap();
    assert!(artifacts.rust_source.contains("prepared_root_source"));
    assert!(artifacts.rust_source.contains("prepared_root_assign"));
    assert!(artifacts.rust_source.contains(".with_root_scope("));
    assert_eq!(
        mf_compiler::CompiledWorkflow::from_json(&artifacts.plan_json).unwrap(),
        plan
    );
}

#[cfg(feature = "codegen")]
#[test]
fn generated_variables_match_memory_without_sources() {
    use mf_compiler::{CompileRequest, SupportPackages, compile_project};
    use std::{fs, process::Command};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("variables.json");
    let binary = directory.path().join("runner");
    let build = directory.path().join("build");
    let mut value = gate_writes(loop_output_updates());
    value["nodes"][0]["config"]["parameters"]["value"]["type"] = json!("any");
    value["dependencies"] = json!({
        "core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")},
        "code":{"package":"mfn-code", "path":common::crates_dir().join("builtin-nodes/code")}
    });
    let expected = run(value.clone());
    fs::write(&path, value.to_string()).unwrap();
    compile_project(&CompileRequest {
        definition: &path,
        output: &binary,
        locked: false,
        build_dir: Some(&build),
        support: &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    })
    .unwrap();
    fs::remove_file(&path).unwrap();
    for _ in 0..2 {
        let output = Command::new(&binary).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            expected
        );
    }
    let output = Command::new(&binary)
        .args(["--params.value", "-1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"count":0,"maybe":null,"child":99,"loop":12,"loop_observed":12})
    );
    let output = Command::new(&binary)
        .args(["--params.value", "\"wrong\""])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("expected int"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut exited = exited_loop_output_updates();
    exited["dependencies"] = value["dependencies"].clone();
    let expected = run(exited.clone());
    fs::write(&path, exited.to_string()).unwrap();
    compile_project(&CompileRequest {
        definition: &path,
        output: &binary,
        locked: false,
        build_dir: Some(&build),
        support: &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    })
    .unwrap();
    fs::remove_file(&path).unwrap();
    let output = Command::new(&binary).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        expected
    );
}
