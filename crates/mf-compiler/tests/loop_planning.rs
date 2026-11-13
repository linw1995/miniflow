#[path = "common/snapshots.rs"]
mod snapshots;
use snapshots::Snapshots;
#[path = "fixtures/observation_capture.rs"]
mod capture;
mod common;
extern crate mfn_code as _;
extern crate mfn_core as _;

use mf_compiler::{
    CompileRequest, ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeRegistration,
    NodeRegistry, Outputs, PortSpec, SupportPackages, TaskNode, ValueType, WorkflowDefinition,
    WorkflowDefinitionVersion, compile_definition, compile_project, execute_compiled,
    instantiate_compiled, plan_definition,
};
use mf_telemetry::{
    event::{Event, LoopPassOutcome, LoopStopReason},
    identity::RunId,
};
use serde_json::{Value, json};
use std::{error::Error, fs, process::Command};

fn find_source<'a, T: Error + 'static>(error: &'a (dyn Error + 'static)) -> Option<&'a T> {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(source) = error.downcast_ref::<T>() {
            return Some(source);
        }
        current = error.source();
    }
    None
}

fn prepared_loop_body() -> (Value, mf_runtime::PreparedSubgraph) {
    let body = json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [{"id": "tick", "kind": "builtin.constant", "config": {"value": true}}],
    });
    let nodes = parse(body.clone())
        .nodes
        .into_iter()
        .map(|node| mf_telemetry::event::NodeIdentity {
            id: node.id.to_string(),
            kind: node.kind,
            path: Vec::new(),
        })
        .collect();
    (
        body,
        mf_runtime::PreparedSubgraph::new(nodes, Vec::new(), |_| Ok(Outputs::new())),
    )
}

struct OmitOnSecond;

struct WrongType;

impl TaskNode for WrongType {
    fn execute(
        &self,
        _: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok((Outputs::from([("value".into(), json!("wrong").into())])).into())
    }
}

impl TaskNode for OmitOnSecond {
    fn execute(
        &self,
        inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        if inputs["index"] == json!(0) {
            Ok((Outputs::from([("value".into(), json!(1).into())])).into())
        } else {
            Ok(Outputs::new().into())
        }
    }
}

fn omit_factory(_: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = OmitOnSecond;
    let metadata = mf_runtime::NodeMetadata::new(mf_runtime::NodePorts {
        inputs: vec![PortSpec::new("index", ValueType::Int64, true)],
        outputs: vec![PortSpec::new("value", ValueType::Int64, true)],
    });
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(node)),
        metadata,
    ))
}

fn wrong_type_factory(_: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = WrongType;
    let metadata = mf_runtime::NodeMetadata::new(mf_runtime::NodePorts {
        inputs: vec![],
        outputs: vec![PortSpec::new("value", ValueType::Any, true)],
    });
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(node)),
        metadata,
    ))
}

inventory::submit! { NodeRegistration { kind: "test.omit_on_second", factory: mf_runtime::NodeFactory::Plain(omit_factory) } }

inventory::submit! { NodeRegistration { kind: "test.wrong_type", factory: mf_runtime::NodeFactory::Plain(wrong_type_factory) } }

fn definition() -> Value {
    json!({
        "version": "2026-09-29",
        "dependencies": {},
        "nodes": [
            {"id": "seed", "kind": "builtin.constant", "config": {"value": 0}},
            {
                "id": "repeat",
                "kind": "workflow.loop",
                "config": {
                    "max_iterations": 5,
                    "variables": [{"name": "count", "type": "int"}],
                    "until": {"variable": "count", "operator": "gte", "value": 3},
                    "body": {"version":"2026-09-29", "dependencies":{},
                        "nodes": [
                            {"id": "assign", "kind": "workflow.loop_assign", "config": {"variable": "count"}},
                            {"id": "increment", "kind": "builtin.code", "config": {
                                "language": "cel", "inputs": {"count": "int"},
                                "code": {"next": "count + 1"}
                            }}
                        ],
                        "edges": [
                            {"from_node": "%loop", "from_output": "count", "to_node": "increment", "to_input": "count"},
                            {"from_node": "increment", "from_output": "next", "to_node": "assign", "to_input": "value"}
                        ]
                    }
                }
            }
        ],
        "edges": [{"from_node": "seed", "from_output": "value", "to_node": "repeat", "to_input": "count"}],
        "outputs": [{"name": "count", "node": "repeat", "port": "count"}]
    })
}

fn parse(value: Value) -> WorkflowDefinition {
    serde_json::from_value(value).unwrap()
}

#[test]
fn public_structural_order_validates_root_scope_syntax() {
    let mut value = definition();
    let order = mf_compiler::structural_order(&parse(value.clone())).unwrap();
    assert_eq!(
        order.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
        ["seed", "repeat"]
    );
    value["nodes"].as_array_mut().unwrap().push(json!({
        "id": "outside", "kind": "workflow.exit_loop"
    }));
    let error = mf_compiler::structural_order(&parse(value)).unwrap_err();
    assert!(matches!(error,
        mf_compiler::WorkflowCompileError::InvalidScope { path, message }
            if path == "[\"outside\"]" && message == "exit requires a state scope"));
}

#[test]
fn exit_configuration_must_be_empty_inside_an_actual_loop() {
    let mut value = exit_definition();
    let exit = value["nodes"][1]["config"]["body"]["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["id"] == "exit")
        .unwrap();
    exit["config"] = json!({"unexpected": true});
    let error = plan_definition(&parse(value)).unwrap_err();
    let mf_compiler::WorkflowCompileError::ChildGraph {
        definition_id,
        source,
    } = &error
    else {
        panic!("expected the enclosing Loop owner: {error}");
    };
    assert_eq!(definition_id.as_str(), "repeat");
    assert!(matches!(source.as_ref(),
        mf_compiler::WorkflowCompileError::InvalidScope { path, message }
            if path == "[\"repeat\",\"exit\"]" && message == "exit config must be empty"));
}

#[test]
fn plans_and_type_checks_nested_loop_body() {
    let definition = parse(definition());
    assert_eq!(definition.version, WorkflowDefinitionVersion::V2026_09_29);
    let plan = plan_definition(&definition).unwrap();
    let body = &plan.graph.definition.nodes[1]
        .flow_parameters(&["body"])
        .unwrap()["body"];
    let authored_nodes: Vec<_> = body
        .nodes
        .iter()
        .filter(|node| node.id.as_str() != "%loop")
        .collect();
    assert_eq!(authored_nodes[0].id.as_str(), "increment");
    assert_eq!(authored_nodes[1].id.as_str(), "assign");
    compile_definition(&definition, &NodeRegistry::from_inventory().unwrap()).unwrap();
}

#[test]
fn loop_requires_a_registered_declaration() {
    let mut value = definition();
    value["nodes"].as_array_mut().unwrap().remove(0);
    value["edges"] = json!([]);
    let error = compile_definition(&parse(value), &NodeRegistry::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("workflow.loop"), "{error}");
}

#[test]
fn rejects_old_schema_and_invalid_loop_structure() {
    let mut value = definition();
    value["version"] = json!("2026-09-26");
    assert!(
        plan_definition(&parse(value))
            .unwrap_err()
            .to_string()
            .contains("2026-09-29")
    );

    let mut value = definition();
    value["nodes"][1]["config"]["max_iterations"] = json!(0);
    let authored = parse(value);
    // The planner only lowers structure; the prepared Loop factory owns the bound.
    plan_definition(&authored).unwrap();
    let error =
        compile_definition(&authored, &NodeRegistry::from_inventory().unwrap()).unwrap_err();
    assert!(
        matches!(&error, mf_compiler::WorkflowCompileError::NodeConstruction { definition_id, kind, .. }
            if definition_id.as_str() == "repeat" && kind == mf_runtime::LOOP_KIND),
        "{error}"
    );
    assert!(matches!(
        find_source::<mfn_core::LoopBuildError>(&error).unwrap(),
        mfn_core::LoopBuildError::InvalidMaximum
    ));
    assert!(
        error
            .to_string()
            .contains("max_iterations must be in 1..=1000")
    );

    let mut value = definition();
    value["nodes"][1]["config"]["body"]["edges"][0]["from_node"] = json!("seed");
    assert!(
        plan_definition(&parse(value))
            .unwrap_err()
            .to_string()
            .contains("unknown source")
    );

    let mut value = definition();
    value["nodes"][1]["config"]["body"]["control_edges"] = json!([{
        "from_node": "assign", "from_output": "done", "to_node": "increment"
    }]);
    assert!(
        plan_definition(&parse(value))
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );

    for kind in ["workflow.loop_assign", "workflow.exit_loop"] {
        let mut value = definition();
        value["nodes"].as_array_mut().unwrap().push(json!({
            "id": "outside", "kind": kind,
            "config": if kind == "workflow.loop_assign" {
                json!({"variable": "count"})
            } else {
                json!({})
            }
        }));
        let error = plan_definition(&parse(value)).unwrap_err();
        let expected = if kind == mf_runtime::LOOP_ASSIGN_KIND {
            "assignment requires a state scope"
        } else {
            "exit requires a state scope"
        };
        assert!(
            matches!(&error, mf_compiler::WorkflowCompileError::InvalidScope { path, message }
                if path == "[\"outside\"]" && message == expected),
            "{error}"
        );
    }
}

#[test]
fn loop_variable_validation_matches_planner_and_runtime_construction() {
    for (variables, expected) in [
        (json!([]), "variables must not be empty"),
        (
            json!([{"name": "index", "type": "int"}]),
            "invalid or reserved variable name",
        ),
        (
            json!([
                {"name": "count", "type": "int"},
                {"name": "count", "type": "int"}
            ]),
            "duplicate variable",
        ),
        (
            json!([{"name": "count", "type": "unsupported"}]),
            "unsupported type",
        ),
    ] {
        let mut value = definition();
        value["nodes"][1]["config"]["variables"] = variables.clone();
        let error = plan_definition(&parse(value)).unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");

        let (body, prepared_body) = prepared_loop_body();
        let config = json!({
            "max_iterations": 5,
            "variables": variables,
            "body": body,
        });
        let error = mf_compiler::instantiate_subgraphs_with_metadata(
            &NodeRegistry::from_inventory().unwrap(),
            "repeat",
            mf_runtime::LOOP_KIND,
            &config.to_string(),
            [("body".into(), prepared_body)].into(),
        )
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains(expected), "{error}");
    }

    let (body, prepared_body) = prepared_loop_body();
    let mut complete_definition = definition()["nodes"][1]["config"].clone();
    complete_definition["body"] = body;
    let registry = NodeRegistry::from_inventory().unwrap();
    let prepared = mf_compiler::instantiate_subgraphs_with_metadata(
        &registry,
        "repeat",
        mf_runtime::LOOP_KIND,
        &complete_definition.to_string(),
        [("body".into(), prepared_body)].into(),
    )
    .unwrap();
    assert_eq!(prepared.metadata.ports.inputs[0].name, "count");

    // An empty prepared body is a Loop business-policy failure, not a lowering error.
    complete_definition["body"]["nodes"] = json!([]);
    let mut authored = definition();
    authored["nodes"][1]["config"] = complete_definition.clone();
    plan_definition(&parse(authored)).unwrap();
    let error = mf_compiler::instantiate_subgraphs_with_metadata(
        &registry,
        "repeat",
        mf_runtime::LOOP_KIND,
        &complete_definition.to_string(),
        [(
            "body".into(),
            mf_runtime::PreparedSubgraph::new(Vec::new(), Vec::new(), |_| Ok(Outputs::new())),
        )]
        .into(),
    )
    .err()
    .unwrap();
    assert!(matches!(
        find_source::<mfn_core::LoopBuildError>(&error).unwrap(),
        mfn_core::LoopBuildError::EmptyBody
    ));
    assert!(
        error
            .to_string()
            .contains("Loop body must contain at least one node")
    );
}

#[test]
fn validates_body_plugins_and_loop_port_types() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = definition();
    value["nodes"][1]["config"]["body"]["nodes"][1]["kind"] = json!("missing.plugin");
    assert!(
        compile_definition(&parse(value), &registry)
            .unwrap_err()
            .to_string()
            .contains("missing.plugin")
    );

    let mut inactive = exit_definition();
    inactive["nodes"][1]["config"]["body"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "invalid_inactive", "kind": "missing.plugin"}));
    inactive["nodes"][1]["config"]["body"]["control_edges"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from_node": "route", "from_output": "stop", "to_node": "invalid_inactive"}));
    assert!(
        compile_definition(&parse(inactive), &registry)
            .unwrap_err()
            .to_string()
            .contains("missing.plugin")
    );

    let mut value = definition();
    value["nodes"][1]["config"]["variables"][0]["type"] = json!("string");
    value["nodes"][1]["config"]["until"] = Value::Null;
    let error = compile_definition(&parse(value), &registry)
        .unwrap_err()
        .to_string();
    assert!(error.contains("cannot connect"), "{error}");

    let mut value = definition();
    value["nodes"][1]["config"]["body"]["nodes"][0]["config"]["variable"] = json!("unknown");
    assert!(
        plan_definition(&parse(value))
            .unwrap_err()
            .to_string()
            .contains("unknown variable")
    );
}

#[test]
fn initial_literal_does_not_specialize_later_passes() {
    let mut value = definition();
    value["nodes"][0]["config"]["value"] = json!("initial");
    value["nodes"][1]["config"]["variables"][0]["type"] = json!("any");
    value["nodes"][1]["config"]["until"] = Value::Null;
    compile_definition(&parse(value), &NodeRegistry::from_inventory().unwrap()).unwrap();
}

#[test]
fn generated_loop_matches_in_memory_without_build_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let definition_path = directory.path().join("loop.json");
    let executable = directory.path().join("loop-runner");
    let build = directory.path().join("build");
    for (index, (mut value, expected)) in [
        (definition(), json!({"count": 3})),
        (nested_definition(), json!({"count": 2})),
        (exit_definition(), json!({"count": 2})),
        (
            defaults_and_inputs_definition(),
            json!({"count": 4, "total": 4}),
        ),
        (
            output_updates_definition(),
            json!({"count":3, "previous":2, "unchanged":"seed"}),
        ),
        (
            nested_output_updates_definition(),
            json!({"count":3, "previous":2, "unchanged":"seed"}),
        ),
        (
            optional_output_updates_definition(),
            json!({"count":1, "previous":1, "unchanged":"seed"}),
        ),
        (
            exited_output_updates_definition(),
            json!({"count":0, "previous":1, "unchanged":"seed"}),
        ),
        (skipped_definition(), json!({})),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(run_in_memory(value.clone()).unwrap(), expected);
        value["dependencies"] = json!({
            "core": {"package": "mfn-core", "path": common::crates_dir().join("builtin-nodes/core")},
            "code": {"package": "mfn-code", "path": common::crates_dir().join("builtin-nodes/code")}
        });
        fs::write(&definition_path, value.to_string()).unwrap();
        let support = SupportPackages::Local {
            crates_dir: common::crates_dir(),
        };
        let request = CompileRequest {
            definition: &definition_path,
            output: &executable,
            locked: false,
            build_dir: Some(&build),
            support: &support,
        };
        compile_project(&request).unwrap();
        let result = Command::new(&executable).output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap(),
            expected
        );
        if index == 0 {
            assert_invalid_loop_edits_preserve_installed_binary_and_lock(&request, value);
        }
    }
    fs::remove_file(&definition_path).unwrap();
    let result = Command::new(&executable).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        json!({})
    );

    let mut value = budget_definition();
    value["dependencies"] = json!({
        "core": {"package": "mfn-core", "path": common::crates_dir().join("builtin-nodes/core")},
        "code": {"package": "mfn-code", "path": common::crates_dir().join("builtin-nodes/code")}
    });
    fs::write(&definition_path, value.to_string()).unwrap();
    compile_project(&CompileRequest {
        definition: &definition_path,
        output: &executable,
        locked: false,
        build_dir: Some(&build),
        support: &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    })
    .unwrap();
    let result = Command::new(&executable).output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("budget"));
}

fn assert_invalid_loop_edits_preserve_installed_binary_and_lock(
    request: &CompileRequest<'_>,
    mut value: Value,
) {
    let definition_path = request.definition;
    let executable = request.output;
    let installed = fs::read(executable).unwrap();
    let lock_path = definition_path.with_extension("lock");
    let lock = fs::read(&lock_path).unwrap();
    value["nodes"][1]["config"]["max_iterations"] = json!(0);
    fs::write(definition_path, value.to_string()).unwrap();
    let error: mf_compiler::PipelineError = compile_project(request).unwrap_err();
    assert_eq!(error.stage, "Cargo build");
    assert_eq!(
        error.project,
        fs::canonicalize(request.build_dir.unwrap()).unwrap()
    );
    let source = find_source::<std::io::Error>(&error).unwrap();
    assert_eq!(source.kind(), std::io::ErrorKind::Other);
    assert!(source.to_string().contains("Cargo build failed with"));
    // Cargo owns the subprocess boundary, so the pipeline retains a typed I/O
    // status source. Capture the generated build's diagnostic separately to prove
    // the failure came from the Loop factory rather than a different build error.
    let build = mf_compiler::cargo_command(&error.project)
        .args(["build", "--release", "--offline", "--locked"])
        .output()
        .unwrap();
    assert!(!build.status.success());
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("failed to construct node `repeat` of kind `workflow.loop`")
            && stderr.contains("max_iterations must be in 1..=1000"),
        "{stderr}"
    );
    assert_eq!(fs::read(executable).unwrap(), installed);
    assert_eq!(fs::read(&lock_path).unwrap(), lock);

    value["nodes"][1]["config"]["max_iterations"] = json!(5);
    value["nodes"][0] = json!({
        "id": "seed",
        "kind": "builtin.code",
        "config": {"language": "cel", "inputs": {}, "code": {"value": "0"}}
    });
    value["dependencies"]
        .as_object_mut()
        .unwrap()
        .remove("core");
    fs::write(definition_path, value.to_string()).unwrap();
    let error = compile_project(request).unwrap_err();
    // Receiver contracts must now be discovered before generation. Omitting
    // the provider is rejected at discovery, without touching installed output.
    assert_eq!(error.stage, "Flow source discovery");
    assert_eq!(fs::read(executable).unwrap(), installed);
    assert_eq!(fs::read(&lock_path).unwrap(), lock);
}

fn run_in_memory(value: Value) -> Result<Value, String> {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(value), &registry).map_err(|error| error.to_string())?;
    let flow = instantiate_compiled(&plan, &registry).map_err(|error| error.to_string())?;
    let output = flow.execute().map_err(|error| error.to_string())?;
    Ok(serde_json::to_value(output).unwrap())
}

#[test]
fn documented_loop_example_executes_with_default_and_explicit_step() {
    let section = include_str!("../../../docs/workflows.md")
        .split_once("## Structured Loop\n")
        .unwrap()
        .1;
    let example = section
        .split_once("```json\n")
        .unwrap()
        .1
        .split_once("\n```")
        .unwrap()
        .0;
    let definition = WorkflowDefinition::from_json(example).unwrap();
    assert_eq!(
        definition,
        WorkflowDefinition::from_json(include_str!("../../../examples/loop-outputs.json")).unwrap()
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    for (inputs, expected) in [
        (json!({}), json!({"count":3})),
        (json!({"params.step":2}), json!({"count":4})),
        (json!({}), json!({"count":3})),
    ] {
        let outputs = flow
            .execute_with_inputs(mf_runtime::WorkflowArguments::try_from(inputs).unwrap())
            .unwrap();
        assert_eq!(serde_json::to_value(outputs).unwrap(), expected);
    }
}

fn defaults_and_inputs_definition() -> Value {
    json!({
        "version": "2026-11-11",
        "dependencies": {},
        "nodes": [
            {"id": "seed", "kind": "builtin.constant", "config": {"value": 2}},
            {"id": "repeat", "kind": "workflow.loop", "config": {
                "max_iterations": 5,
                "inputs": {"step": "int"},
                "variables": [
                    {"name": "count", "type": "int", "initial": 0},
                    {"name": "total", "type": "int", "initial": 0}
                ],
                "until": {"variable": "count", "operator": "gte", "value": 4},
                "body": {
                    "version": "2026-10-28",
                    "dependencies": {},
                    "nodes": [
                        {"id": "increment", "kind": "builtin.code", "config": {
                            "language": "cel", "inputs": {"count": "int", "step": "int"},
                            "code": {"next": "count + step"}
                        }, "inputs": {
                            "count": {"from": {"node": "%loop", "port": "count"}},
                            "step": {"from": {"node": "%loop", "port": "step"}}
                        }},
                        {"id": "assign_count", "kind": "workflow.loop_assign",
                         "config": {"variable": "count"},
                         "inputs": {"value": {"from": {"node": "increment", "port": "next"}}}},
                        {"id": "sum", "kind": "builtin.code", "config": {
                            "language": "cel", "inputs": {"total": "int", "step": "int"},
                            "code": {"next": "total + step"}
                        }, "inputs": {
                            "total": {"from": {"node": "%loop", "port": "total"}},
                            "step": {"from": {"node": "%loop", "port": "step"}}
                        }},
                        {"id": "assign_total", "kind": "workflow.loop_assign",
                         "config": {"variable": "total"},
                         "inputs": {"value": {"from": {"node": "sum", "port": "next"}}}}
                    ]
                }
            }, "inputs": {"step": {"from": {"node": "seed", "port": "value"}}}}
        ],
        "outputs": [
            {"name": "count", "node": "repeat", "port": "count"},
            {"name": "total", "node": "repeat", "port": "total"}
        ]
    })
}

#[test]
fn loop_defaults_and_shared_inputs_are_optional_and_read_only() {
    assert_eq!(
        run_in_memory(defaults_and_inputs_definition()).unwrap(),
        json!({"count": 4, "total": 4})
    );

    // An explicit Loop input overrides the variable default.
    let mut bound = defaults_and_inputs_definition();
    bound["nodes"][1]["inputs"]["count"] = json!({"from": {"node": "seed", "port": "value"}});
    assert_eq!(
        run_in_memory(bound).unwrap(),
        json!({"count": 4, "total": 2})
    );

    fn expect_error(edit: impl FnOnce(&mut Value), expected: &str) {
        let mut value = defaults_and_inputs_definition();
        edit(&mut value);
        let error = compile_definition(&parse(value), &NodeRegistry::from_inventory().unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
    }
    expect_error(
        |value| value["nodes"][1]["config"]["until"]["variable"] = json!("step"),
        "unknown condition variable `step`",
    );
    expect_error(
        |value| {
            value["nodes"][1]["config"]["body"]["nodes"][1]["config"]["variable"] = json!("step");
        },
        "unknown variable `step`",
    );
    expect_error(
        |value| value["nodes"][1]["config"]["inputs"]["count"] = json!("int"),
        "Loop inputs must be nonblank, distinct from variables, and not index",
    );
    expect_error(
        |value| value["nodes"][1]["config"]["variables"][0]["initial"] = json!("zero"),
        "invalid initial value for Loop variable `count`",
    );
    expect_error(
        |value| value["version"] = json!("2026-10-28"),
        "Loop inputs and variable defaults require definition version 2026-11-11",
    );
}

fn output_updates_definition() -> Value {
    json!({
        "version":"2026-11-13",
        "nodes":[{"id":"repeat", "kind":"workflow.loop", "config":{
            "max_iterations":5,
            "variables":[
                {"name":"count", "type":"int", "initial":0},
                {"name":"previous", "type":"int", "initial":-1},
                {"name":"unchanged", "type":"string", "initial":"seed"}
            ],
            "until":{"variable":"count", "operator":"gte", "value":3},
            "body":{"nodes":[
                {"id":"increment", "kind":"builtin.code", "config":{
                    "language":"cel", "inputs":{"count":"int"}, "code":{"next":"count + 1"}
                }, "inputs":{"count":{"from":{"node":"%loop", "port":"count"}}}}
            ], "outputs":[
                {"name":"count", "node":"increment", "port":"next"},
                {"name":"previous", "node":"%loop", "port":"count"}
            ]}
        }}],
        "outputs":[
            {"name":"count", "node":"repeat", "port":"count"},
            {"name":"previous", "node":"repeat", "port":"previous"},
            {"name":"unchanged", "node":"repeat", "port":"unchanged"}
        ]
    })
}

fn optional_output_updates_definition() -> Value {
    let mut value = output_updates_definition();
    let config = &mut value["nodes"][0]["config"];
    config["max_iterations"] = json!(2);
    config.as_object_mut().unwrap().remove("until");
    config["body"]["nodes"].as_array_mut().unwrap().push(json!({
        "id":"route", "kind":"builtin.if_else", "config":{"branches":[{
            "id":"first", "condition":{"source":{"output":"%loop.index", "path":""},
                "operator":"eq", "value":0}
        }]}
    }));
    config["body"]["edges"] = json!([
        {"from_node":"%loop", "from_output":"index", "to_node":"route"},
        {"from_node":"route", "from_output":"first", "to_node":"increment"}
    ]);
    config["body"]["outputs"][0]["optional"] = json!(true);
    value
}

fn exited_output_updates_definition() -> Value {
    let mut value = output_updates_definition();
    let body = &mut value["nodes"][0]["config"]["body"];
    body["nodes"].as_array_mut().unwrap().extend([
        json!({"id":"mark", "kind":"workflow.loop_assign", "config":{"variable":"previous"},
            "inputs":{"value":{"from":{"node":"increment", "port":"next"}}}}),
        json!({"id":"exit", "kind":"workflow.exit_loop"}),
        json!({"id":"unvisited", "kind":"builtin.identity", "inputs":{
            "input":{"from":{"node":"increment", "port":"next"}}
        }}),
    ]);
    // Both wait for the immediate write; the canonical order puts exit first.
    body["edges"] = json!([
        {"from_node":"mark", "from_output":"done", "to_node":"exit"},
        {"from_node":"mark", "from_output":"done", "to_node":"unvisited"}
    ]);
    body["outputs"] = json!([{"name":"count", "node":"unvisited", "port":"value"}]);
    value
}

fn nested_output_updates_definition() -> Value {
    let mut value = output_updates_definition();
    let mut inner = value["nodes"][0].clone();
    inner["id"] = json!("inner");
    inner["config"]["max_iterations"] = json!(1);
    inner["config"].as_object_mut().unwrap().remove("until");
    inner["inputs"] = json!({"count":{"from":{"node":"%loop", "port":"count"}}});
    let body = &mut value["nodes"][0]["config"]["body"];
    body["nodes"] = json!([inner]);
    body["outputs"][0] = json!({"name":"count", "node":"inner", "port":"count"});
    value
}

#[test]
fn body_outputs_preserve_native_capabilities_and_secret_labels() {
    use mf_runtime::{FlowValue, PreparedSubgraph, ValueRef, WorkflowArguments};
    let definition = parse(json!({"version":"2026-11-13", "nodes":[
        {"id":"params", "kind":"builtin.parameters", "config":{"parameters":{
            "function":{"type":"flow", "secret":true}
        }}},
        {"id":"repeat", "kind":"workflow.loop", "config":{
            "max_iterations":2,
            "inputs":{"replacement":"flow"},
            "variables":[{"name":"function", "type":"flow"}],
            "body":{"nodes":[
                {"id":"pick", "kind":"builtin.identity", "inputs":{
                    "input":{"from":{"node":"%loop", "port":"replacement"}}
                }}
            ], "outputs":[{"name":"function", "node":"pick", "port":"value"}]}
        }, "inputs":{
            "function":{"from":{"node":"params", "port":"function"}},
            "replacement":{"from":{"node":"params", "port":"function"}}
        }}
    ], "outputs":[{"name":"function", "node":"repeat", "port":"function"}]}));
    let capability = ValueRef::from(
        FlowValue::new(PreparedSubgraph::new(
            Vec::new(),
            vec![PortSpec::new("content", ValueType::String, true)],
            |_| {
                Ok(Outputs::from([(
                    "content".into(),
                    "private-capability-result".into(),
                )]))
            },
        ))
        .unwrap(),
    )
    .into_secret();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let snapshots = Snapshots::memory();
    let mut context = ExecutionContext::default();
    context.set_snapshot_recorder(snapshots.recorder());
    context.set_workflow_arguments(WorkflowArguments(Inputs::from([(
        "params.function".into(),
        capability.clone(),
    )])));
    let output = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_in_context(&mut context)
        .unwrap();
    assert!(output["function"].is_secret());
    assert_eq!(output["function"].as_flow(), capability.as_flow());
    let result = output["function"]
        .as_flow()
        .unwrap()
        .invoke("tool", Default::default(), &mut ExecutionContext::default())
        .unwrap();
    assert_eq!(result["content"], json!("private-capability-result"));
    for entry in snapshots.history() {
        let snapshot = entry.snapshot.node(&entry.scope, &entry.node).unwrap();
        assert!(
            !serde_json::to_string(snapshot)
                .unwrap()
                .contains("private-capability-result")
        );
    }
}

#[test]
fn body_outputs_commit_together_before_until_and_survive_saved_plans() {
    let value = output_updates_definition();
    let expected = json!({"count":3, "previous":2, "unchanged":"seed"});
    assert_eq!(run_in_memory(value.clone()).unwrap(), expected);
    assert_eq!(
        run_in_memory(nested_output_updates_definition()).unwrap(),
        expected
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let saved = mf_compiler::CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap();
    saved.manifest(&registry).unwrap().validate().unwrap();
    assert_eq!(
        serde_json::to_value(
            instantiate_compiled(&saved, &registry)
                .unwrap()
                .execute()
                .unwrap()
        )
        .unwrap(),
        expected
    );
    assert_eq!(
        run_in_memory(optional_output_updates_definition()).unwrap(),
        json!({"count":1, "previous":1, "unchanged":"seed"})
    );
}

#[test]
fn exit_discards_body_outputs_but_retains_immediate_assignments() {
    assert_eq!(
        run_in_memory(exited_output_updates_definition()).unwrap(),
        json!({"count":0, "previous":1, "unchanged":"seed"})
    );
}

#[test]
fn body_output_updates_reject_old_versions_bad_targets_types_and_conflicting_writes() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for target in ["missing", "index", "input"] {
        let mut value = output_updates_definition();
        value["nodes"][0]["config"]["inputs"] = json!({"input":"int"});
        value["nodes"][0]["config"]["body"]["outputs"][0]["name"] = json!(target);
        let error = compile_definition(&parse(value), &registry).unwrap_err();
        assert!(matches!(find_source::<mfn_core::LoopBuildError>(&error),
            Some(mfn_core::LoopBuildError::UnknownBodyOutput { variable }) if variable == target));
    }
    let mut wrong_type = output_updates_definition();
    wrong_type["nodes"][0]["config"]["body"]["outputs"][0] =
        json!({"name":"count", "node":"%loop", "port":"unchanged"});
    let error = compile_definition(&parse(wrong_type), &registry).unwrap_err();
    assert!(matches!(
        find_source::<mfn_core::LoopBuildError>(&error),
        Some(mfn_core::LoopBuildError::BodyOutputType { .. })
    ));

    let mut conflict = exited_output_updates_definition();
    conflict["nodes"][0]["config"]["body"]["outputs"][0]["name"] = json!("previous");
    let error = compile_definition(&parse(conflict), &registry).unwrap_err();
    assert!(matches!(
        find_source::<mfn_core::LoopBuildError>(&error),
        Some(mfn_core::LoopBuildError::ConflictingBodyOutput { .. })
    ));

    let mut old = output_updates_definition();
    old["nodes"][0]["config"]["body"]["version"] = json!("2026-11-11");
    let error = compile_definition(&parse(old), &registry).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Loop body output updates require definition version 2026-11-13")
    );

    let mut old = compile_definition(&parse(output_updates_definition()), &registry).unwrap();
    old.graph
        .children
        .get_mut(&"repeat".into())
        .unwrap()
        .get_mut("body")
        .unwrap()
        .definition
        .version = WorkflowDefinitionVersion::V2026_11_11;
    assert!(instantiate_compiled(&old, &registry).is_err());
}

#[test]
fn required_skips_and_missing_producers_are_not_silent_state_retention() {
    let mut value = optional_output_updates_definition();
    value["nodes"][0]["config"]["body"]["outputs"][0]["optional"] = json!(false);
    assert!(
        run_in_memory(value)
            .unwrap_err()
            .contains("requires skipped output")
    );

    let mut value = output_updates_definition();
    value["nodes"][0]["config"]["body"]["nodes"][0] = json!({
        "id":"increment", "kind":"test.omit_on_second",
        "inputs":{"index":{"from":{"node":"%loop", "port":"index"}}}
    });
    value["nodes"][0]["config"]["body"]["outputs"][0] = json!({
        "name":"count", "node":"increment", "port":"value", "optional":true
    });
    assert!(run_in_memory(value).is_err());
}

#[test]
fn invalid_body_update_retains_typed_sources_and_marks_the_pass_failed() {
    let mut value = output_updates_definition();
    let config = &mut value["nodes"][0]["config"];
    config["variables"][0]["type"] = json!({"list":{"map":"int"}});
    config["variables"][0]["initial"] = json!([]);
    config.as_object_mut().unwrap().remove("until");
    config["body"] = json!({"nodes":[{"id":"wrong", "kind":"test.wrong_type"}],
        "outputs":[{"name":"count", "node":"wrong", "port":"value"}]});
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let harness = capture::Harness::new(true);
    let observation = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    let error = execute_compiled(&plan, &registry, Some(observation)).unwrap_err();
    assert!(find_source::<mf_runtime::TypeMismatch>(&error).is_some());
    assert!(
        error
            .to_string()
            .contains("invalid Loop body update `count`")
    );
    assert!(harness.records().iter().any(|record| matches!(
        record.decode().unwrap().event,
        Event::LoopPassFinished {
            outcome: LoopPassOutcome::Failed,
            ..
        }
    )));
    assert!(
        !harness
            .records()
            .iter()
            .any(|record| matches!(record.decode().unwrap().event,
        Event::NodeFinished { node, outcome: mf_telemetry::event::Outcome::Succeeded, .. }
            if node.id == "repeat"))
    );
}

#[test]
fn stops_at_maximum_and_immediate_exit() {
    let mut value = definition();
    value["nodes"][1]["config"]["until"] = Value::Null;
    assert_eq!(run_in_memory(value.clone()).unwrap(), json!({"count": 5}));

    assert_eq!(
        run_in_memory(exit_definition()).unwrap(),
        json!({"count": 2})
    );
}

fn exit_definition() -> Value {
    let mut value = definition();
    value["nodes"][1]["config"]["until"] = Value::Null;
    let body = &mut value["nodes"][1]["config"]["body"];
    body["nodes"].as_array_mut().unwrap().extend([
        json!({"id": "route", "kind": "builtin.if_else", "config": {"branches": [{
            "id": "stop", "condition": {"source": {"output": "increment.next", "path": ""},
            "operator": "eq", "value": 2}
        }]}}),
        json!({"id": "exit", "kind": "workflow.exit_loop"}),
        json!({"id": "override", "kind": "builtin.constant", "config": {"value": 999}}),
        json!({"id": "zz_after", "kind": "workflow.loop_assign", "config": {"variable": "count"}}),
    ]);
    body["edges"].as_array_mut().unwrap().push(json!({
        "from_node": "override", "from_output": "value", "to_node": "zz_after", "to_input": "value"
    }));
    body["control_edges"] = json!([
        {"from_node": "assign", "from_output": "done", "to_node": "route"},
        {"from_node": "route", "from_output": "stop", "to_node": "exit"},
        {"from_node": "route", "from_output": "stop", "to_node": "zz_after"}
    ]);
    value
}

#[test]
fn compiled_scope_relationships_execute_independently_of_instruction_labels() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut authored = exit_definition();
    authored["nodes"][1]["config"]["variables"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "observed", "type": "int"}));
    authored["edges"].as_array_mut().unwrap().push(json!({
        "from_node": "seed", "from_output": "value", "to_node": "repeat", "to_input": "observed"
    }));
    authored["outputs"].as_array_mut().unwrap().push(json!({
        "name": "observed", "node": "repeat", "port": "observed"
    }));
    let authored_body = &mut authored["nodes"][1]["config"]["body"];
    authored_body["nodes"].as_array_mut().unwrap().extend([
        json!({"id": "read_updated", "kind": "builtin.identity"}),
        json!({"id": "capture_updated", "kind": "workflow.loop_assign", "config": {"variable": "observed"}}),
    ]);
    authored_body["edges"].as_array_mut().unwrap().extend([
        json!({"from_node": "%loop", "from_output": "count", "to_node": "read_updated", "to_input": "input"}),
        json!({"from_node": "read_updated", "from_output": "value", "to_node": "capture_updated", "to_input": "value"}),
    ]);
    authored_body["control_edges"][0] = json!({
        "from_node": "capture_updated", "from_output": "done", "to_node": "route"
    });
    authored_body["control_edges"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "from_node": "assign", "from_output": "done", "to_node": "read_updated"
        }));
    let mut plan = compile_definition(&parse(authored), &registry).unwrap();
    let expected = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap();
    // This is a fixed semantic expectation, not merely equivalence to the old path:
    // a source read after a write must see the newly committed value on both passes.
    assert_eq!(expected["count"], json!(2));
    assert_eq!(expected["observed"], json!(2));
    let body = plan
        .graph
        .children
        .get_mut(&"repeat".into())
        .unwrap()
        .get_mut("body")
        .unwrap();
    let relations = body.relations.clone();
    let frame = relations.scope.as_ref().unwrap();
    assert!(!frame.writes.is_empty());
    assert!(!frame.exits.is_empty());
    let original_input = frame.input.clone();
    let renamed_input = mf_runtime::DefinitionId::from("%actual-input");
    let rename = |id: &mut mf_runtime::DefinitionId| {
        if id == &original_input {
            *id = renamed_input.clone();
        }
    };
    for node in &mut body.definition.nodes {
        if relations.instruction(&node.id).is_some() {
            node.kind = "unlinked.scope_instruction".into();
            node.config = json!({"opaque": "not an instruction configuration"});
        }
        rename(&mut node.id);
    }
    for edge in &mut body.definition.edges {
        rename(&mut edge.from_node);
        rename(&mut edge.to_node);
    }
    for edge in &mut body.definition.control_edges {
        rename(&mut edge.from_node);
        rename(&mut edge.to_node);
    }
    for edge in &mut body.definition.order_edges {
        rename(&mut edge.from_node);
        rename(&mut edge.to_node);
    }
    for output in &mut body.definition.outputs {
        rename(&mut output.node);
    }
    for id in &mut body.execution_order {
        rename(id);
    }
    body.relations.scope.as_mut().unwrap().input = renamed_input;
    assert_eq!(body.relations.scope.as_ref().unwrap().writes, frame.writes);
    assert_eq!(body.relations.scope.as_ref().unwrap().exits, frame.exits);
    let saved = mf_compiler::CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap();
    assert_eq!(
        instantiate_compiled(&saved, &registry)
            .unwrap()
            .execute()
            .unwrap(),
        expected
    );
    assert_eq!(expected["count"], json!(2));
    let manifest = saved.manifest(&registry).unwrap();
    manifest.validate().unwrap();
    mf_compiler::describe_compiled(&saved).unwrap();
    #[cfg(feature = "codegen")]
    {
        let execution = saved.generate_execution_plans(&registry).unwrap();
        let generated = saved.generate_artifacts().unwrap();
        for source in [&execution.rust_source, &generated.rust_source] {
            assert!(source.contains("%actual-input"));
        }
        assert!(generated.rust_source.contains("with_relations"));
        let runner = saved.generate_runner_artifacts().unwrap();
        assert!(
            runner
                .rust_source
                .contains("mf_prepare_generated_workflow!")
        );
        assert_eq!(runner.plan_json, generated.plan_json);
    }

    // The identical label outside a relationship endpoint remains a provider node.
    let ordinary = parse(json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [{"id": "ordinary", "kind": "unlinked.scope_instruction"}]
    }));
    let plan = plan_definition(&ordinary).unwrap();
    assert_eq!(plan.graph.relations, mf_runtime::GraphRelations::default());
    let error = instantiate_compiled(&plan, &registry).err().unwrap();
    assert!(
        matches!(error, mf_compiler::WorkflowCompileError::UnknownNodeKind { definition_id, kind }
        if definition_id.as_str() == "ordinary" && kind == "unlinked.scope_instruction")
    );
}

#[test]
fn skipped_assignment_preserves_previous_value() {
    let mut value = definition();
    value["nodes"][1]["config"]["until"] = Value::Null;
    value["nodes"][1]["config"]["max_iterations"] = json!(2);
    let body = &mut value["nodes"][1]["config"]["body"];
    body["nodes"].as_array_mut().unwrap().push(json!({
        "id": "route", "kind": "builtin.if_else", "config": {"branches": [{
            "id": "skip", "condition": {"source": {"output": "%loop.index", "path": ""},
            "operator": "eq", "value": 0}
        }]}
    }));
    body["control_edges"] = json!([
        {"from_node": "%loop", "from_output": "index", "to_node": "route"},
        {"from_node": "route", "from_output": "else", "to_node": "assign"}
    ]);
    assert_eq!(run_in_memory(value).unwrap(), json!({"count": 1}));
}

fn nested_definition() -> Value {
    let mut value = definition();
    let inner = value["nodes"][1]["config"].clone();
    value["nodes"][1]["config"]["until"] = Value::Null;
    value["nodes"][1]["config"]["max_iterations"] = json!(2);
    let mut inner = inner;
    inner["until"] = Value::Null;
    inner["max_iterations"] = json!(1);
    value["nodes"][1]["config"]["body"] = json!({"version":"2026-09-29", "dependencies":{},
        "nodes": [
            {"id": "inner", "kind": "workflow.loop", "config": inner},
            {"id": "assign_outer", "kind": "workflow.loop_assign", "config": {"variable": "count"}}
        ],
        "edges": [
            {"from_node": "%loop", "from_output": "count", "to_node": "inner", "to_input": "count"},
            {"from_node": "inner", "from_output": "count", "to_node": "assign_outer", "to_input": "value"}
        ]
    });
    value
}

#[test]
fn nested_loops_restore_parent_state_and_share_step_budget() {
    assert_eq!(
        run_in_memory(nested_definition()).unwrap(),
        json!({"count": 2})
    );
    let plan = plan_definition(&parse(nested_definition())).unwrap();
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    assert_eq!(description.loop_bodies.len(), 2);
    assert_eq!(description.loop_bodies[0].path, ["repeat"]);
    assert_eq!(description.loop_bodies[1].path, ["repeat", "inner"]);
    let error = run_in_memory(budget_definition()).unwrap_err();
    assert!(
        error.contains("budget") && error.contains("inner"),
        "{error}"
    );
}

fn budget_definition() -> Value {
    let mut value = nested_definition();
    let inner = &mut value["nodes"][1]["config"]["body"]["nodes"][0]["config"];
    inner["max_iterations"] = json!(1000);
    inner["body"] = json!({"version":"2026-09-29", "dependencies":{},
        "nodes": [{"id": "tick", "kind": "builtin.constant", "config": {"value": true}}]
    });
    value["nodes"][1]["config"]["max_iterations"] = json!(1000);
    value
}

#[test]
fn later_pass_cannot_read_an_omitted_prior_output() {
    let mut value = definition();
    value["nodes"][1]["config"]["until"] = Value::Null;
    value["nodes"][1]["config"]["max_iterations"] = json!(2);
    value["nodes"][1]["config"]["body"] = json!({"version":"2026-09-29", "dependencies":{},
        "nodes": [
            {"id": "probe", "kind": "test.omit_on_second"},
            {"id": "sink", "kind": "builtin.identity"}
        ],
        "edges": [
            {"from_node": "%loop", "from_output": "index", "to_node": "probe", "to_input": "index"},
            {"from_node": "probe", "from_output": "value", "to_node": "sink", "to_input": "input"}
        ]
    });
    let error = run_in_memory(value).unwrap_err();
    assert!(
        error.contains("pass 1") && error.contains("probe.value"),
        "{error}"
    );
}

#[test]
fn skips_the_whole_loop_when_a_control_dependency_is_inactive() {
    assert_eq!(run_in_memory(skipped_definition()).unwrap(), json!({}));
}

fn skipped_definition() -> Value {
    let mut value = definition();
    value["nodes"].as_array_mut().unwrap().push(json!({
        "id": "route", "kind": "builtin.if_else", "config": {"branches": [{
            "id": "run", "condition": {"source": {"output": "seed.value", "path": ""},
            "operator": "eq", "value": 1}
        }]}
    }));
    value["control_edges"] = json!([
        {"from_node": "seed", "from_output": "value", "to_node": "route"},
        {"from_node": "route", "from_output": "run", "to_node": "repeat"}
    ]);
    value["outputs"][0]["optional"] = json!(true);
    value
}

#[test]
fn invalid_assignment_does_not_publish_partial_loop_output() {
    let mut value = definition();
    value["nodes"][1]["config"]["body"] = json!({"version":"2026-09-29", "dependencies":{},
        "nodes": [
            {"id": "wrong", "kind": "test.wrong_type"},
            {"id": "assign", "kind": "workflow.loop_assign", "config": {"variable": "count"}}
        ],
        "edges": [{"from_node": "wrong", "from_output": "value", "to_node": "assign", "to_input": "value"}]
    });
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let mut context = ExecutionContext::default();
    let error = flow
        .execute_in_context(&mut context)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("pass 0") && error.contains("expected int64"),
        "{error}"
    );
    assert!(context.output("repeat.count").is_err());
}

#[test]
fn loop_observation_identifies_each_pass_and_body_invocation() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = definition();
    value["nodes"][1]["config"]["max_iterations"] = json!(3);
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let harness = capture::Harness::new(true);
    let observation = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    let output = execute_compiled(&plan, &registry, Some(observation)).unwrap();
    assert_eq!(output["count"], json!(3));
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    assert_eq!(description.loop_bodies.len(), 1);
    let records = harness.records();
    assert!(
        records
            .iter()
            .all(|record| record.schema_version().unwrap() == mf_telemetry::EVENT_SCHEMA_VERSION)
    );
    let mut pass_starts = 0;
    let mut pass_finishes = 0;
    let mut increments = Vec::new();
    let mut summary = None;
    for record in &records {
        let event = record.decode().unwrap();
        event.validate_for(&description).unwrap();
        match event.event {
            Event::LoopPassStarted { path, .. } => {
                assert_eq!(path.last().unwrap().node_id(), "repeat");
                pass_starts += 1;
            }
            Event::LoopPassFinished { path, outcome, .. } => {
                assert_eq!(path.last().unwrap().node_id(), "repeat");
                assert_eq!(outcome, LoopPassOutcome::Completed);
                pass_finishes += 1;
            }
            Event::NodeFinished {
                node, loop_summary, ..
            } if node.id == "increment" => {
                increments.push(node.path.last().unwrap().index().get());
                assert!(loop_summary.is_none());
            }
            Event::NodeFinished {
                node, loop_summary, ..
            } if node.id == "repeat" => {
                summary = loop_summary;
            }
            _ => {}
        }
        assert!(!record.body.to_string().contains("\"count\":3"));
    }
    assert_eq!(
        (pass_starts, pass_finishes),
        (3, 3),
        "{:?}",
        records
            .iter()
            .map(|record| &record.event_name)
            .collect::<Vec<_>>()
    );
    assert_eq!(increments, vec![0, 1, 2]);
    let summary = summary.unwrap();
    assert_eq!(summary.pass_count.get(), 3);
    assert_eq!(summary.reason, LoopStopReason::Condition);
    let spans = harness.spans.get_finished_spans().unwrap();
    let loop_span = spans
        .iter()
        .find(|span| {
            span.attributes.iter().any(|attribute| {
                attribute.key.as_str() == "mf.node.id" && attribute.value.to_string() == "repeat"
            })
        })
        .unwrap();
    let passes: Vec<_> = spans
        .iter()
        .filter(|span| span.name == "mf.loop.pass")
        .collect();
    assert_eq!(passes.len(), 3);
    for pass in passes {
        assert_eq!(pass.parent_span_id, loop_span.span_context.span_id());
        let index: i64 = pass
            .attributes
            .iter()
            .find(|attribute| attribute.key.as_str() == "mf.loop.index")
            .unwrap()
            .value
            .to_string()
            .parse()
            .unwrap();
        assert!(
            pass.attributes
                .iter()
                .any(|attribute| attribute.key.as_str() == "mf.loop.id"
                    && attribute.value.to_string() == "repeat")
        );
        assert!(
            pass.attributes
                .iter()
                .any(|attribute| attribute.key.as_str() == "mf.loop.outcome"
                    && attribute.value.to_string() == "completed")
        );
        let child = spans
            .iter()
            .find(|span| {
                span.parent_span_id == pass.span_context.span_id()
                    && span.attributes.iter().any(|attribute| {
                        attribute.key.as_str() == "mf.node.id"
                            && attribute.value.to_string() == "increment"
                    })
            })
            .unwrap();
        for record in &records {
            let event = record.decode().unwrap();
            match event.event {
                Event::LoopPassStarted { path, .. } | Event::LoopPassFinished { path, .. }
                    if path.last().unwrap().index().get() == index =>
                {
                    assert_eq!(
                        record.trace_context.as_ref().unwrap().span_id,
                        pass.span_context.span_id().to_string()
                    );
                }
                Event::NodeStarted { node, .. }
                    if node.id == "increment"
                        && node.path.last().unwrap().index().get() == index =>
                {
                    assert_eq!(
                        record.trace_context.as_ref().unwrap().span_id,
                        child.span_context.span_id().to_string()
                    );
                }
                _ => {}
            }
        }
    }
}

#[test]
fn loop_observation_distinguishes_exit_skip_and_nested_paths() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (value, expected) in [
        (exit_definition(), json!({"count": 2})),
        (skipped_definition(), json!({})),
        (nested_definition(), json!({"count": 2})),
    ] {
        let plan = compile_definition(&parse(value), &registry).unwrap();
        let description = mf_compiler::describe_compiled(&plan).unwrap();
        let harness = capture::Harness::new(true);
        let observation = plan
            .start_observation(&harness.observer(), RunId::new())
            .unwrap();
        let output = execute_compiled(&plan, &registry, Some(observation)).unwrap();
        assert_eq!(serde_json::to_value(output).unwrap(), expected);
        let events: Vec<_> = harness
            .records()
            .iter()
            .map(|record| {
                let event = record.decode().unwrap();
                event.validate_for(&description).unwrap();
                event.event
            })
            .collect();
        if expected == json!({}) {
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Event::LoopPassStarted { .. }))
            );
            assert!(events.iter().any(|event| matches!(event,
                Event::NodeSkipped { node, .. } if node.id == "repeat")));
        } else if description.loop_bodies.len() == 2 {
            assert!(events.iter().any(|event| matches!(event,
                Event::NodeFinished { node, .. }
                    if node.id == "increment" && node.path.len() == 2)));
            let spans = harness.spans.get_finished_spans().unwrap();
            let inner_passes: Vec<_> = spans
                .iter()
                .filter(|span| {
                    span.name == "mf.loop.pass"
                        && span.attributes.iter().any(|attribute| {
                            attribute.key.as_str() == "mf.loop.id"
                                && attribute.value.to_string() == "inner"
                        })
                })
                .collect();
            assert_eq!(inner_passes.len(), 2);
            for pass in inner_passes {
                let inner = spans
                    .iter()
                    .find(|span| span.span_context.span_id() == pass.parent_span_id)
                    .unwrap();
                assert!(
                    inner
                        .attributes
                        .iter()
                        .any(|attribute| attribute.key.as_str() == "mf.node.id"
                            && attribute.value.to_string() == "inner")
                );
                let outer = spans
                    .iter()
                    .find(|span| span.span_context.span_id() == inner.parent_span_id)
                    .unwrap();
                assert_eq!(outer.name, "mf.loop.pass");
                assert!(spans.iter().any(|span| {
                    span.parent_span_id == pass.span_context.span_id()
                        && span.attributes.iter().any(|attribute| {
                            attribute.key.as_str() == "mf.node.id"
                                && attribute.value.to_string() == "increment"
                        })
                }));
            }
        } else {
            let outcomes: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    Event::LoopPassFinished { outcome, .. } => Some(*outcome),
                    _ => None,
                })
                .collect();
            assert_eq!(
                outcomes,
                [LoopPassOutcome::Completed, LoopPassOutcome::Exit]
            );
            assert!(
                harness
                    .spans
                    .get_finished_spans()
                    .unwrap()
                    .iter()
                    .any(|span| span.name == "mf.loop.pass"
                        && span
                            .attributes
                            .iter()
                            .any(|attribute| attribute.key.as_str() == "mf.loop.outcome"
                                && attribute.value.to_string() == "exit"))
            );
            assert!(events.iter().any(|event| matches!(event,
                Event::NodeSkipped { node, causes, .. }
                    if node.id == "exit" && causes.iter().any(|cause|
                        cause.source_node() == "route" && cause.source_output() == Some("stop")))));
            assert!(events.iter().any(|event| matches!(event,
                Event::NodeFinished { node, loop_summary: Some(summary), .. }
                    if node.id == "repeat" && summary.reason == LoopStopReason::Exit)));
        }
    }
}

#[test]
fn failed_body_emits_a_failed_pass_and_no_loop_success() {
    let mut value = definition();
    value["nodes"][1]["config"]["body"] = json!({"version":"2026-09-29", "dependencies":{},
        "nodes": [
            {"id": "wrong", "kind": "test.wrong_type"},
            {"id": "assign", "kind": "workflow.loop_assign", "config": {"variable": "count"}}
        ],
        "edges": [{"from_node": "wrong", "from_output": "value", "to_node": "assign", "to_input": "value"}]
    });
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    let harness = capture::Harness::new(true);
    let observation = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    assert!(execute_compiled(&plan, &registry, Some(observation)).is_err());
    let events: Vec<_> = harness
        .records()
        .iter()
        .map(|record| {
            let event = record.decode().unwrap();
            event.validate_for(&description).unwrap();
            event.event
        })
        .collect();
    assert!(events.iter().any(|event| matches!(
        event,
        Event::LoopPassFinished {
            outcome: LoopPassOutcome::Failed,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(event,
        Event::NodeFinished { node, outcome: mf_telemetry::event::Outcome::Failed, .. }
            if node.id == "assign" && node.path.len() == 1)));
    assert!(!events.iter().any(|event| matches!(event,
        Event::NodeFinished { node, outcome: mf_telemetry::event::Outcome::Succeeded, .. }
            if node.id == "repeat")));
    let spans = harness.spans.get_finished_spans().unwrap();
    let pass = spans
        .iter()
        .find(|span| span.name == "mf.loop.pass")
        .unwrap();
    assert!(matches!(
        pass.status,
        opentelemetry::trace::Status::Error { .. }
    ));
    assert!(
        pass.attributes
            .iter()
            .any(|attribute| attribute.key.as_str() == "mf.loop.outcome"
                && attribute.value.to_string() == "failed")
    );
}

#[test]
fn loop_snapshots_keep_each_body_node_span_context() {
    use opentelemetry::trace::TraceContextExt;
    use std::sync::{Arc, Mutex};
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(definition()), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let harness = capture::Harness::new(true);
    let observation = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    let captures = Arc::new(Mutex::new(Vec::new()));
    let received = Arc::clone(&captures);
    ExecutionContext::run(Some(observation), |state| {
        state.set_snapshot_recorder(mf_runtime::SnapshotRecorder::new(move |event| {
            if event.node.as_ref() == "increment" {
                let context = opentelemetry::Context::current();
                received.lock().unwrap().push((
                    event.scope.last().unwrap().index(),
                    context.span().span_context().span_id(),
                ));
            }
        }));
        flow.execute_in_context(state)
    })
    .unwrap();
    let spans = harness.spans.get_finished_spans().unwrap();
    let captures = captures.lock().unwrap();
    assert_eq!(captures.len(), 6);
    for (index, span_id) in captures.iter() {
        let node = spans
            .iter()
            .find(|span| span.span_context.span_id() == *span_id)
            .unwrap();
        let pass = spans
            .iter()
            .find(|span| span.span_context.span_id() == node.parent_span_id)
            .unwrap();
        assert_eq!(pass.name, "mf.loop.pass");
        assert!(
            pass.attributes
                .iter()
                .any(|attribute| attribute.key.as_str() == "mf.loop.index"
                    && attribute.value.to_string() == index.get().to_string())
        );
    }
}

#[test]
fn unsampled_loop_passes_keep_lifecycle_delivery_complete() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(definition()), &registry).unwrap();
    let harness = capture::Harness::new(false);
    let observation = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    assert_eq!(
        execute_compiled(&plan, &registry, Some(observation)).unwrap()["count"],
        json!(3)
    );
    assert!(harness.spans.get_finished_spans().unwrap().is_empty());
    let records = harness.records();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.event_name == "mf.loop.pass.started")
            .count(),
        3
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record.event_name == "mf.loop.pass.finished")
            .count(),
        3
    );
    assert_eq!(records.last().unwrap().event_name, "mf.workflow.finished");
}

#[test]
fn abandoned_loop_pass_ends_its_span_without_fabricating_a_finish() {
    use mf_telemetry::event::LoopPathEntry;
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(definition()), &registry).unwrap();
    let harness = capture::Harness::new(true);
    let mut observation = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    let _workflow = observation.enter();
    let node = observation.begin_node("repeat").unwrap();
    let _node = node.enter();
    let pass = observation
        .begin_loop_pass(vec![LoopPathEntry::Loop {
            loop_id: "repeat".into(),
            index: mf_telemetry::Count::ZERO,
        }])
        .unwrap();
    drop(pass);
    let spans = harness.spans.get_finished_spans().unwrap();
    let pass = spans
        .iter()
        .find(|span| span.name == "mf.loop.pass")
        .unwrap();
    assert!(matches!(
        pass.status,
        opentelemetry::trace::Status::Error { .. }
    ));
    let records = harness.records();
    assert!(
        records
            .iter()
            .any(|record| record.event_name == "mf.loop.pass.started")
    );
    assert!(
        !records
            .iter()
            .any(|record| record.event_name == "mf.loop.pass.finished")
    );
}

#[test]
fn legacy_observation_keeps_pass_spans_without_loop_lifecycle_records() {
    use mf_telemetry::event::{LoopPathEntry, NodeIdentity};
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(definition()), &registry).unwrap();
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    let harness = capture::Harness::new(true);
    let mut observation = harness
        .observer()
        .start(
            description.workflow_id,
            RunId::new(),
            vec![NodeIdentity {
                id: "repeat".into(),
                kind: "workflow.loop".into(),
                path: Vec::new(),
            }],
        )
        .unwrap();
    let _workflow = observation.enter();
    let node = observation.begin_node("repeat").unwrap();
    let _node = node.enter();
    assert!(observation.begin_loop_pass(Vec::new()).is_none());
    assert!(
        observation
            .begin_loop_pass(vec![LoopPathEntry::Call {
                call_id: "call".into()
            }])
            .is_none()
    );
    let path = vec![LoopPathEntry::Loop {
        loop_id: "repeat".into(),
        index: mf_telemetry::Count::ZERO,
    }];
    let mut pass = observation.begin_loop_pass(path.clone()).unwrap();
    pass.finish(LoopPassOutcome::Completed);
    pass.finish(LoopPassOutcome::Failed);
    observation.finish(None);
    assert!(observation.begin_loop_pass(path).is_none());
    let spans = harness.spans.get_finished_spans().unwrap();
    let pass = spans
        .iter()
        .find(|span| span.name == "mf.loop.pass")
        .unwrap();
    assert!(
        pass.attributes
            .iter()
            .any(|attribute| attribute.key.as_str() == "mf.loop.outcome"
                && attribute.value.to_string() == "completed")
    );
    assert!(!matches!(
        pass.status,
        opentelemetry::trace::Status::Error { .. }
    ));
    assert!(
        !harness
            .records()
            .iter()
            .any(|record| record.event_name.starts_with("mf.loop.pass."))
    );
}

#[test]
fn snapshots_preserve_inputs_and_outputs_from_all_loop_passes() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(definition()), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let recorder = Snapshots::memory();
    let mut context = ExecutionContext::default();
    context.set_snapshot_recorder(recorder.recorder());
    assert_eq!(
        flow.execute_in_context(&mut context).unwrap()["count"],
        json!(3)
    );
    let root = recorder.current();
    for index in 0..3 {
        let path = vec![mf_telemetry::event::LoopPathEntry::Loop {
            loop_id: "repeat".into(),
            index: mf_telemetry::Count::try_from(index).unwrap(),
        }];
        let increment = root.node(&path, "increment").unwrap();
        let assign = root.node(&path, "assign").unwrap();
        assert_eq!(increment.inputs["count"], json!(index));
        assert_eq!(increment.outputs["next"], json!(index + 1));
        assert!(increment.outputs["next"].ptr_eq(&assign.inputs["value"]));
    }
    let first = recorder
        .history()
        .into_iter()
        .find(|entry| {
            entry.node.as_ref() == "increment"
                && entry
                    .snapshot
                    .node(&entry.scope, "increment")
                    .unwrap()
                    .outcome
                    == mf_runtime::SnapshotOutcome::Succeeded
        })
        .unwrap();
    assert_eq!(
        first
            .snapshot
            .node(&first.scope, "increment")
            .unwrap()
            .outputs["next"],
        json!(1)
    );
    assert!(
        first
            .snapshot
            .node(&[], "repeat")
            .unwrap()
            .outputs
            .as_object()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn observed_preparation_attributes_damaged_loop_graphs_to_the_root_owner() {
    use mf_compiler::WorkflowCompileError;
    let registry = NodeRegistry::from_inventory().unwrap();
    let valid = compile_definition(&parse(definition()), &registry).unwrap();
    for case in ["input relationship", "child factory", "write relationship"] {
        let harness = capture::Harness::new(true);
        let mut observation = valid
            .start_observation(&harness.observer(), RunId::new())
            .unwrap();
        let mut damaged = valid.clone();
        match case {
            "input relationship" => {
                damaged
                    .graph
                    .children
                    .get_mut(&"repeat".into())
                    .unwrap()
                    .get_mut("body")
                    .unwrap()
                    .relations
                    .scope
                    .as_mut()
                    .unwrap()
                    .input = "missing-input".into();
            }
            "child factory" => {
                let child = damaged
                    .graph
                    .children
                    .get_mut(&"repeat".into())
                    .unwrap()
                    .get_mut("body")
                    .unwrap()
                    .definition
                    .nodes
                    .iter_mut()
                    .find(|node| node.id.as_str() == "increment")
                    .unwrap();
                child.kind = "builtin.constant".into();
                child.config = json!(17);
            }
            "write relationship" => {
                damaged
                    .graph
                    .children
                    .get_mut(&"repeat".into())
                    .unwrap()
                    .get_mut("body")
                    .unwrap()
                    .relations
                    .scope
                    .as_mut()
                    .unwrap()
                    .writes
                    .insert("assign".into(), "unknown".into());
            }
            _ => unreachable!(),
        }
        let error = mf_compiler::instantiate_compiled_with_observation(
            &damaged,
            &registry,
            Some(&mut observation),
        )
        .err()
        .unwrap();
        match (&error, case) {
            (
                WorkflowCompileError::ChildGraph {
                    definition_id,
                    source,
                },
                "input relationship" | "write relationship",
            ) => {
                assert_eq!(definition_id.as_str(), "repeat");
                let expected = if case == "input relationship" {
                    "scope instruction relationships must be disjoint and reference known nodes"
                } else {
                    "scope write target must name an available state output and value"
                };
                assert!(matches!(source.as_ref(),
                    WorkflowCompileError::InvalidScope { path, message }
                        if path == "[\"repeat\"]" && message == expected));
            }
            (
                WorkflowCompileError::ChildGraph {
                    definition_id,
                    source,
                },
                "child factory",
            ) => {
                assert_eq!(definition_id.as_str(), "repeat");
                assert!(matches!(source.as_ref(),
                    WorkflowCompileError::NodeConstruction { definition_id, kind, source }
                        if definition_id.as_str() == "increment" && kind == "builtin.constant"
                            && matches!(source, NodeBuildError::InvalidConfiguration { .. })));
                assert!(find_source::<serde_json::Error>(&error).unwrap().is_data());
            }
            _ => panic!("{case}: unexpected preparation error: {error}"),
        }
        observation.finish(Some(&error));
        let records = harness.records();
        assert_eq!(
            records
                .iter()
                .map(|record| record.event_name.as_str())
                .collect::<Vec<_>>(),
            [
                "mf.workflow.started",
                "mf.node.finished",
                "mf.workflow.finished"
            ],
            "{case}"
        );
        assert_eq!(records[1].attributes["mf.node.id"], "repeat");
        assert_eq!(records[1].attributes["mf.failure.phase"], "preparation");
        assert_eq!(records[1].attributes["mf.outcome"], "failed");
        assert_eq!(records[1].body["produced_ports"], json!([]));
        assert_eq!(records[2].body["failure_node_id"], "repeat");
        assert_eq!(records[2].attributes["mf.failure.phase"], "preparation");
    }
}

#[test]
fn observation_preserves_description_and_structural_order_errors() {
    use std::error::Error;
    let mut plan = plan_definition(&parse(definition())).unwrap();
    let body = &mut plan
        .graph
        .children
        .get_mut(&"repeat".into())
        .unwrap()
        .get_mut("body")
        .unwrap()
        .definition;
    body.edges
        .iter_mut()
        .find(|edge| edge.to_node.as_str() == "assign")
        .unwrap()
        .from_node = "missing".into();
    let harness = capture::Harness::new(true);
    let error = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap_err();
    let mf_compiler::DescriptionError::DescriptionOrder { path, .. } = &error else {
        panic!("expected a structural ordering error: {error}");
    };
    assert_eq!(path, &["repeat"]);
    assert!(
        error
            .source()
            .unwrap()
            .is::<Box<mf_compiler::WorkflowCompileError>>()
    );
    assert!(harness.records().is_empty());
}

#[test]
fn descriptions_reject_damaged_loop_relationships_with_typed_owners() {
    use std::error::Error;
    let valid = plan_definition(&parse(definition())).unwrap();
    // A malformed raw body is rejected while validating already-resolved authoring
    // input. Raw config is opaque during offline inspection: compiled children are the
    // authority, and a typed CompiledGraph cannot contain a string definition.
    let mut malformed = definition();
    malformed["nodes"][1]["config"]["body"] = json!("not a workflow");
    let error = mf_compiler::plan_resolved_definition(
        &parse(malformed),
        &NodeRegistry::from_inventory().unwrap(),
    )
    .unwrap_err();
    assert!(
        matches!(&error, mf_compiler::WorkflowCompileError::FlowParameters { definition_id, .. }
            if definition_id.as_str() == "repeat"),
        "{error}"
    );
    assert!(matches!(
        error.source().unwrap().downcast_ref::<mf_runtime::FlowParameterError>().unwrap(),
        mf_runtime::FlowParameterError::Json { parameter, .. } if parameter == "body"
    ));
    assert!(find_source::<serde_json::Error>(&error).unwrap().is_data());
    let mut malformed = serde_json::to_value(&valid).unwrap();
    malformed["graph"]["children"]["repeat"]["body"]["definition"] = json!("not a workflow");
    let error = mf_compiler::CompiledWorkflow::from_json(&malformed.to_string()).unwrap_err();
    assert!(
        matches!(&error, mf_compiler::PlanError::Parse { .. }),
        "{error}"
    );
    assert!(error.source().unwrap().is::<serde_json::Error>());

    let mut old_version = parse(definition());
    old_version.version = WorkflowDefinitionVersion::V2026_09_26;
    let error = mf_compiler::plan_resolved_definition(
        &old_version,
        &NodeRegistry::from_inventory().unwrap(),
    )
    .unwrap_err();
    assert!(
        matches!(error, mf_compiler::WorkflowCompileError::InvalidScope { path, message }
        if path == "[\"repeat\"]"
            && message == "scope constructs require definition version 2026-09-29")
    );

    // Persisted instructions are relationships, not authoring kind/config tokens.
    for (case, expected) in [
        (
            "overlapping roles",
            "scope instruction relationships must be disjoint and reference known nodes",
        ),
        (
            "unknown write target",
            "scope write target must name an available state output and value",
        ),
        (
            "missing input",
            "scope instruction relationships must be disjoint and reference known nodes",
        ),
    ] {
        let mut damaged = valid.clone();
        let frame = damaged
            .graph
            .children
            .get_mut(&"repeat".into())
            .unwrap()
            .get_mut("body")
            .unwrap()
            .relations
            .scope
            .as_mut()
            .unwrap();
        match case {
            "overlapping roles" => frame.input = "assign".into(),
            "unknown write target" => {
                frame.writes.insert("assign".into(), "unknown".into());
            }
            "missing input" => frame.input = "missing".into(),
            _ => unreachable!(),
        }
        let error = mf_compiler::describe_compiled(&damaged).unwrap_err();
        let mf_compiler::DescriptionError::DescriptionOrder { path, source } = &error else {
            panic!("{case}: expected an ordering error: {error}");
        };
        assert!(path.is_empty(), "{case}: {error}");
        let mf_compiler::WorkflowCompileError::ChildGraph {
            definition_id,
            source: child,
        } = source.as_ref()
        else {
            panic!("{case}: expected a child graph error: {error}");
        };
        assert_eq!(definition_id.as_str(), "repeat");
        assert!(
            source
                .source()
                .unwrap()
                .is::<Box<mf_compiler::WorkflowCompileError>>()
        );
        assert!(
            matches!(child.as_ref(), mf_compiler::WorkflowCompileError::InvalidScope { path, message }
            if path == "[\"repeat\"]" && message == expected),
            "{case}: {error}"
        );
        assert!(child.source().is_none());
        assert!(find_source::<serde_json::Error>(&error).is_none());
        assert!(error.to_string().contains(expected), "{case}: {error}");
        assert!(
            error
                .source()
                .unwrap()
                .is::<Box<mf_compiler::WorkflowCompileError>>()
        );
    }
}

#[test]
fn resolved_authoring_missing_loop_bodies_fail_at_the_receiving_factory() {
    use std::error::Error;
    let registry = NodeRegistry::from_inventory().unwrap();
    for nested in [false, true] {
        let mut authored = parse(definition());
        let owner = authored
            .nodes
            .iter_mut()
            .find(|node| node.id.as_str() == "repeat")
            .unwrap();
        let expected_owner = if nested {
            // The outer graph is valid; the error belongs to this nested authoring node.
            owner.config["body"]["nodes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id": "inner", "kind": "workflow.loop"}));
            "inner"
        } else {
            owner.config.as_object_mut().unwrap().remove("body");
            "repeat"
        };
        let error = mf_compiler::compile_resolved_definition(&authored, &registry).unwrap_err();
        let leaf = if nested {
            let mf_compiler::WorkflowCompileError::ChildGraph {
                definition_id,
                source,
            } = &error
            else {
                panic!("expected the outer owner context: {error}");
            };
            assert_eq!(definition_id.as_str(), "repeat");
            assert!(
                error
                    .source()
                    .unwrap()
                    .is::<Box<mf_compiler::WorkflowCompileError>>()
            );
            assert!(std::ptr::eq(
                error
                    .source()
                    .unwrap()
                    .downcast_ref::<Box<mf_compiler::WorkflowCompileError>>()
                    .unwrap()
                    .as_ref(),
                source.as_ref()
            ));
            source.as_ref()
        } else {
            &error
        };
        assert!(
            matches!(leaf, mf_compiler::WorkflowCompileError::NodeConstruction { definition_id, kind, source }
                if definition_id.as_str() == expected_owner
                    && kind == mf_runtime::LOOP_KIND
                    && matches!(source, NodeBuildError::InvalidSubgraph { message }
                        if message == "workflow.loop requires a prepared `body` binding")),
            "{error}"
        );
        assert!(leaf.source().unwrap().is::<NodeBuildError>());
        assert!(find_source::<serde_json::Error>(&error).is_none());
        assert!(find_source::<NodeBuildError>(&error).is_some());
    }
}
