#[path = "fixtures/observation_capture.rs"]
mod capture;
mod common;
extern crate mfn_code as _;
extern crate mfn_core as _;

use mf_compiler::{
    CompileRequest, ExecutionContext, Inputs, Node, NodeBuildError, NodeExecutionError,
    NodeRegistration, NodeRegistry, Outputs, PortSpec, SupportPackages, ValueType,
    WorkflowDefinition, WorkflowDefinitionVersion, compile_definition, compile_project,
    execute_compiled, instantiate_compiled, plan_definition,
};
use mf_telemetry::{
    event::{Event, LoopPassOutcome, LoopStopReason},
    identity::RunId,
};
use serde_json::{Value, json};
use std::{fs, process::Command};

struct OmitOnSecond;

struct WrongType;

impl Node for WrongType {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Ok(Outputs::from([("value".into(), json!("wrong"))]))
    }
}

impl Node for OmitOnSecond {
    fn execute(&self, inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        if inputs["index"] == json!(0) {
            Ok(Outputs::from([("value".into(), json!(1))]))
        } else {
            Ok(Outputs::new())
        }
    }
}

fn omit_factory(_: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    Ok(Box::new(OmitOnSecond))
}

fn wrong_type_factory(_: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    Ok(Box::new(WrongType))
}

inventory::submit! { NodeRegistration {
    kind: "test.omit_on_second",
    inputs: &[PortSpec::new("index", ValueType::Int64, true)],
    outputs: &[PortSpec::new("value", ValueType::Int64, true)],
    factory: omit_factory,
} }

inventory::submit! { NodeRegistration {
    kind: "test.wrong_type",
    inputs: &[],
    outputs: &[PortSpec::new("value", ValueType::Any, true)],
    factory: wrong_type_factory,
} }

fn definition() -> Value {
    json!({
        "version": "2026-09-29",
        "dependencies": {},
        "nodes": [
            {"id": "seed", "kind": "builtin.constant", "config": {"value": 0}},
            {
                "id": "repeat",
                "kind": "workflow.loop",
                "loop": {
                    "max_iterations": 5,
                    "variables": [{"name": "count", "type": "int"}],
                    "until": {"variable": "count", "operator": "gte", "value": 3},
                    "body": {
                        "nodes": [
                            {"id": "assign", "kind": "workflow.loop_assign", "config": {"variable": "count"}},
                            {"id": "increment", "kind": "builtin.code", "config": {
                                "language": "cel", "inputs": {"count": "int"},
                                "code": {"next": "count + 1"}
                            }}
                        ],
                        "edges": [
                            {"from_node": "$loop", "from_output": "count", "to_node": "increment", "to_input": "count"},
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
fn plans_and_type_checks_nested_loop_body() {
    let definition = parse(definition());
    assert_eq!(definition.version, WorkflowDefinitionVersion::CURRENT);
    let plan = plan_definition(&definition).unwrap();
    let body = &plan.definition.nodes[1]
        .loop_definition
        .as_ref()
        .unwrap()
        .body;
    assert_eq!(body.nodes[0].id.as_str(), "increment");
    assert_eq!(body.nodes[1].id.as_str(), "assign");
    compile_definition(&definition, &NodeRegistry::from_inventory().unwrap()).unwrap();
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
    value["nodes"][1]["loop"]["max_iterations"] = json!(0);
    assert!(
        plan_definition(&parse(value))
            .unwrap_err()
            .to_string()
            .contains("max_iterations")
    );

    let mut value = definition();
    value["nodes"][1]["loop"]["body"]["edges"][0]["from_node"] = json!("seed");
    assert!(
        plan_definition(&parse(value))
            .unwrap_err()
            .to_string()
            .contains("unknown source")
    );

    let mut value = definition();
    value["nodes"][1]["loop"]["body"]["control_edges"] = json!([{
        "from_node": "assign", "from_output": "done", "to_node": "increment"
    }]);
    assert!(
        plan_definition(&parse(value))
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
}

#[test]
fn validates_body_plugins_and_loop_port_types() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = definition();
    value["nodes"][1]["loop"]["body"]["nodes"][1]["kind"] = json!("missing.plugin");
    assert!(
        compile_definition(&parse(value), &registry)
            .unwrap_err()
            .to_string()
            .contains("missing.plugin")
    );

    let mut value = definition();
    value["nodes"][1]["loop"]["variables"][0]["type"] = json!("string");
    value["nodes"][1]["loop"]["until"] = Value::Null;
    let error = compile_definition(&parse(value), &registry)
        .unwrap_err()
        .to_string();
    assert!(error.contains("cannot connect"), "{error}");

    let mut value = definition();
    value["nodes"][1]["loop"]["body"]["nodes"][0]["config"]["variable"] = json!("unknown");
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
    value["nodes"][1]["loop"]["variables"][0]["type"] = json!("any");
    value["nodes"][1]["loop"]["until"] = Value::Null;
    compile_definition(&parse(value), &NodeRegistry::from_inventory().unwrap()).unwrap();
}

#[test]
fn executes_loop_with_persistent_state_in_memory() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(definition()), &registry).unwrap();
    let output = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(output["count"], json!(3));
}

#[test]
fn generated_loop_matches_in_memory_without_build_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let definition_path = directory.path().join("loop.json");
    let executable = directory.path().join("loop-runner");
    let build = directory.path().join("build");
    for (mut value, expected) in [
        (definition(), json!({"count": 3})),
        (nested_definition(), json!({"count": 2})),
        (exit_definition(), json!({"count": 2})),
        (skipped_definition(), json!({})),
    ] {
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

fn run_in_memory(value: Value) -> Result<Value, String> {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(value), &registry).map_err(|error| error.to_string())?;
    let flow = instantiate_compiled(&plan, &registry).map_err(|error| error.to_string())?;
    let output = flow.execute().map_err(|error| error.to_string())?;
    Ok(serde_json::to_value(output).unwrap())
}

#[test]
fn stops_at_maximum_and_immediate_exit() {
    let mut value = definition();
    value["nodes"][1]["loop"]["until"] = Value::Null;
    assert_eq!(run_in_memory(value.clone()).unwrap(), json!({"count": 5}));

    assert_eq!(
        run_in_memory(exit_definition()).unwrap(),
        json!({"count": 2})
    );
}

fn exit_definition() -> Value {
    let mut value = definition();
    value["nodes"][1]["loop"]["until"] = Value::Null;
    let body = &mut value["nodes"][1]["loop"]["body"];
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
fn skipped_assignment_preserves_previous_value() {
    let mut value = definition();
    value["nodes"][1]["loop"]["until"] = Value::Null;
    value["nodes"][1]["loop"]["max_iterations"] = json!(2);
    let body = &mut value["nodes"][1]["loop"]["body"];
    body["nodes"].as_array_mut().unwrap().push(json!({
        "id": "route", "kind": "builtin.if_else", "config": {"branches": [{
            "id": "skip", "condition": {"source": {"output": "$loop.index", "path": ""},
            "operator": "eq", "value": 0}
        }]}
    }));
    body["control_edges"] = json!([
        {"from_node": "$loop", "from_output": "index", "to_node": "route"},
        {"from_node": "route", "from_output": "else", "to_node": "assign"}
    ]);
    assert_eq!(run_in_memory(value).unwrap(), json!({"count": 1}));
}

fn nested_definition() -> Value {
    let mut value = definition();
    let inner = value["nodes"][1]["loop"].clone();
    value["nodes"][1]["loop"]["until"] = Value::Null;
    value["nodes"][1]["loop"]["max_iterations"] = json!(2);
    let mut inner = inner;
    inner["until"] = Value::Null;
    inner["max_iterations"] = json!(1);
    value["nodes"][1]["loop"]["body"] = json!({
        "nodes": [
            {"id": "inner", "kind": "workflow.loop", "loop": inner},
            {"id": "assign_outer", "kind": "workflow.loop_assign", "config": {"variable": "count"}}
        ],
        "edges": [
            {"from_node": "$loop", "from_output": "count", "to_node": "inner", "to_input": "count"},
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
    let inner = &mut value["nodes"][1]["loop"]["body"]["nodes"][0]["loop"];
    inner["max_iterations"] = json!(1000);
    inner["body"] = json!({
        "nodes": [{"id": "tick", "kind": "builtin.constant", "config": {"value": true}}]
    });
    value["nodes"][1]["loop"]["max_iterations"] = json!(1000);
    value
}

#[test]
fn later_pass_cannot_read_an_omitted_prior_output() {
    let mut value = definition();
    value["nodes"][1]["loop"]["until"] = Value::Null;
    value["nodes"][1]["loop"]["max_iterations"] = json!(2);
    value["nodes"][1]["loop"]["body"] = json!({
        "nodes": [
            {"id": "probe", "kind": "test.omit_on_second"},
            {"id": "sink", "kind": "builtin.identity"}
        ],
        "edges": [
            {"from_node": "$loop", "from_output": "index", "to_node": "probe", "to_input": "index"},
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
    value["nodes"][1]["loop"]["body"] = json!({
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
    let plan = compile_definition(&parse(definition()), &registry).unwrap();
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
            .all(|record| record.schema_version().unwrap() == 2)
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
                assert_eq!(path.last().unwrap().loop_id, "repeat");
                pass_starts += 1;
            }
            Event::LoopPassFinished { path, outcome, .. } => {
                assert_eq!(path.last().unwrap().loop_id, "repeat");
                assert_eq!(outcome, LoopPassOutcome::Completed);
                pass_finishes += 1;
            }
            Event::NodeFinished {
                node, loop_summary, ..
            } if node.id == "increment" => {
                increments.push(node.path.last().unwrap().index.get());
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
    assert!(spans.iter().any(|span| {
        span.attributes.iter().any(|attribute| {
            attribute.key.as_str() == "mf.node.id" && attribute.value.to_string() == "increment"
        }) && span.parent_span_id == loop_span.span_context.span_id()
    }));
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
            assert!(events.iter().any(|event| matches!(event,
                Event::NodeSkipped { node, causes, .. }
                    if node.id == "exit" && causes.iter().any(|cause|
                        cause.source_node == "route" && cause.source_output == "stop"))));
            assert!(events.iter().any(|event| matches!(event,
                Event::NodeFinished { node, loop_summary: Some(summary), .. }
                    if node.id == "repeat" && summary.reason == LoopStopReason::Exit)));
        }
    }
}

#[test]
fn failed_body_emits_a_failed_pass_and_no_loop_success() {
    let mut value = definition();
    value["nodes"][1]["loop"]["body"] = json!({
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
}
