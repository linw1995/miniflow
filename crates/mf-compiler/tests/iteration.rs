#[path = "fixtures/observation_capture.rs"]
mod capture;
mod common;
extern crate mfn_code as _;
extern crate mfn_core as _;

use mf_compiler::{
    CompileRequest, Inputs, Node, NodeBuildError, NodeExecutionError, NodeRegistration,
    NodeRegistry, Outputs, PortSpec, SupportPackages, ValueType, WorkflowDefinition,
    compile_definition, compile_project, execute_compiled, instantiate_compiled, plan_definition,
};
use mf_telemetry::identity::RunId;
use opentelemetry::trace::TraceContextExt;
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

struct MapItem;

impl Node for MapItem {
    fn execute(&self, inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        let item = inputs["item"].as_i64().unwrap();
        if item == 0 {
            return Err(NodeExecutionError::ExecutionFailed {
                message: "zero is not accepted".into(),
            });
        }
        let index = inputs["index"].as_i64().unwrap();
        Ok(Outputs::from([("value".into(), json!(item * 2 + index))]))
    }
}

fn map_factory(_: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    Ok(Box::new(MapItem))
}

inventory::submit! {
    NodeRegistration {
        kind: "test.iteration_map",
        inputs: &[
            PortSpec::new("item", ValueType::Int64, true),
            PortSpec::new("index", ValueType::Int64, true),
        ],
        outputs: &[PortSpec::new("value", ValueType::Int64, true)],
        factory: map_factory,
    }
}

struct TraceProbe;

impl Node for TraceProbe {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        let span = opentelemetry::Context::current()
            .span()
            .span_context()
            .span_id()
            .to_string();
        Ok(Outputs::from([("span".into(), json!(span))]))
    }
}

fn trace_probe_factory(_: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    Ok(Box::new(TraceProbe))
}

inventory::submit! {
    NodeRegistration {
        kind: "test.iteration_trace_probe",
        inputs: &[],
        outputs: &[PortSpec::new("span", ValueType::String, true)],
        factory: trace_probe_factory,
    }
}

fn definition(items: Value, mode: &str, on_error: &str) -> Value {
    json!({
        "version":"2026-09-26",
        "dependencies":{},
        "nodes":[
            {"id":"source","kind":"builtin.constant","config":{"value":items}},
            {"id":"iteration","kind":"builtin.iteration","config":{
                "mode":mode,
                "on_error":on_error,
                "body":{
                    "nodes":[{"id":"map","kind":"test.iteration_map"}],
                    "edges":[
                        {"from_node":"@iteration","from_output":"items","to_node":"map","to_input":"item"},
                        {"from_node":"@iteration","from_output":"index","to_node":"map","to_input":"index"}
                    ],
                    "result":{"node":"map","port":"value"}
                }
            }}
        ],
        "edges":[{"from_node":"source","from_output":"value","to_node":"iteration","to_input":"items"}],
        "outputs":[{"name":"results","node":"iteration","port":"results"}]
    })
}

fn execute(value: Value) -> Result<Value, String> {
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).map_err(|error| error.to_string())?;
    instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .map(|outputs| json!(outputs))
        .map_err(|error| error.to_string())
}

#[test]
fn iteration_collects_values_in_input_order_and_isolates_item_contexts() {
    for mode in ["sequential", "parallel"] {
        let actual = execute(definition(json!([1, 2, 3]), mode, "terminate")).unwrap();
        assert_eq!(actual, json!({"results":[2, 5, 8]}));
        assert_eq!(
            execute(definition(json!([]), mode, "terminate")).unwrap(),
            json!({"results":[]})
        );
    }
}

#[test]
fn iteration_error_policies_preserve_positions_or_remove_failures() {
    for mode in ["sequential", "parallel"] {
        let input = json!([1, 0, 3]);
        let error = execute(definition(input.clone(), mode, "terminate")).unwrap_err();
        assert!(error.contains("iteration item 1") && error.contains("zero is not accepted"));
        assert_eq!(
            execute(definition(input.clone(), mode, "continue_on_error")).unwrap(),
            json!({"results":[2, null, 8]})
        );
        assert_eq!(
            execute(definition(input, mode, "remove_failed")).unwrap(),
            json!({"results":[2, 8]})
        );
    }
}

#[test]
fn iteration_rejects_invalid_body_structure_and_result_ports() {
    let mut invalid = definition(json!([1]), "sequential", "terminate");
    invalid["nodes"][1]["config"]["body"]["nodes"][0]["id"] = json!("@iteration");
    let invalid_definition: WorkflowDefinition = serde_json::from_value(invalid).unwrap();
    assert!(
        plan_definition(&invalid_definition)
            .unwrap_err()
            .to_string()
            .contains("reserved")
    );

    let mut invalid = definition(json!([1]), "sequential", "terminate");
    invalid["nodes"][1]["config"]["body"]["nodes"][0]["kind"] = json!("builtin.iteration");
    let invalid_definition: WorkflowDefinition = serde_json::from_value(invalid).unwrap();
    assert!(
        plan_definition(&invalid_definition)
            .unwrap_err()
            .to_string()
            .contains("nested")
    );

    let mut invalid = definition(json!([1]), "sequential", "terminate");
    invalid["nodes"][1]["config"]["body"]["nodes"][0] = json!({
        "id": "inner_loop",
        "kind": "workflow.loop",
        "loop": {
            "max_iterations": 1,
            "variables": [{"name": "value", "type": "int"}],
            "body": {"nodes": [{"id": "assign", "kind": "workflow.loop_assign", "config": {"variable": "value"}}]}
        }
    });
    let invalid_definition: WorkflowDefinition = serde_json::from_value(invalid).unwrap();
    assert!(
        plan_definition(&invalid_definition)
            .unwrap_err()
            .to_string()
            .contains("unsupported Loop")
    );

    let mut invalid = definition(json!([1]), "sequential", "terminate");
    invalid["nodes"][1]["config"]["body"]["result"]["port"] = json!("missing");
    let definition: WorkflowDefinition = serde_json::from_value(invalid).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let error = compile_definition(&definition, &registry)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("iteration") && error.contains("missing port"),
        "{error}"
    );
}

#[test]
fn iteration_body_order_is_canonical_in_the_compiled_plan() {
    let mut first = definition(json!([1]), "sequential", "terminate");
    first["nodes"][1]["config"]["body"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"unused","kind":"builtin.constant","config":{"value":7}}));
    let mut reordered = first.clone();
    reordered["nodes"][1]["config"]["body"]["nodes"]
        .as_array_mut()
        .unwrap()
        .reverse();
    reordered["nodes"][1]["config"]["body"]["edges"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let registry = NodeRegistry::from_inventory().unwrap();
    let first: WorkflowDefinition = serde_json::from_value(first).unwrap();
    let reordered: WorkflowDefinition = serde_json::from_value(reordered).unwrap();
    assert_eq!(
        compile_definition(&first, &registry)
            .unwrap()
            .to_json()
            .unwrap(),
        compile_definition(&reordered, &registry)
            .unwrap()
            .to_json()
            .unwrap()
    );
}

#[test]
fn parallel_iteration_reports_correlated_item_and_body_node_activity() {
    let definition: WorkflowDefinition = serde_json::from_value(definition(
        json!([1, 0, 3]),
        "parallel",
        "continue_on_error",
    ))
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let harness = capture::Harness::new(true);
    let observation = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    let result = execute_compiled(&plan, &registry, Some(observation)).unwrap();
    assert_eq!(json!(result), json!({"results":[2, null, 8]}));

    let records = harness.records();
    let nested: Vec<_> = records
        .iter()
        .filter(|record| record.scope == "mf.iteration")
        .collect();
    assert_eq!(nested.len(), 12);
    for event in [
        "mf.iteration.item.started",
        "mf.iteration.item.finished",
        "mf.iteration.node.started",
        "mf.iteration.node.finished",
    ] {
        assert_eq!(
            nested
                .iter()
                .filter(|record| record.event_name == event)
                .count(),
            3
        );
    }
    let outer = records
        .iter()
        .find(|record| {
            record.event_name == "mf.node.finished"
                && record.attributes["mf.node.id"] == "iteration"
        })
        .unwrap();
    assert_eq!(outer.attributes["mf.outcome"], "succeeded");
    let failed_item = nested
        .iter()
        .find(|record| {
            record.event_name == "mf.iteration.item.finished"
                && record.attributes["mf.iteration.index"] == 1
        })
        .unwrap();
    assert_eq!(failed_item.attributes["mf.outcome"], "failed");
    let failed_node = nested
        .iter()
        .find(|record| {
            record.event_name == "mf.iteration.node.finished"
                && record.attributes["mf.iteration.index"] == 1
        })
        .unwrap();
    assert_eq!(failed_node.attributes["mf.node.id"], "map");
    assert_eq!(failed_node.attributes["mf.failure.phase"], "execution");
    assert_eq!(failed_node.attributes["mf.outcome"], "failed");
    for record in &nested {
        assert_eq!(record.attributes["mf.iteration.id"], "iteration");
        assert_eq!(
            record.attributes["mf.workflow.id"],
            outer.attributes["mf.workflow.id"]
        );
        assert_eq!(
            record.attributes["mf.run.id"],
            outer.attributes["mf.run.id"]
        );
        assert_eq!(
            record.trace_context.as_ref().unwrap().trace_id,
            outer.trace_context.as_ref().unwrap().trace_id
        );
    }
    let lifecycle: Vec<_> = records
        .iter()
        .filter(|record| record.scope == "mf.workflow")
        .collect();
    for (position, record) in lifecycle.iter().enumerate() {
        assert_eq!(record.decode().unwrap().sequence.get(), position as i64 + 1);
    }

    let spans = harness.spans.get_finished_spans().unwrap();
    let outer_span = spans
        .iter()
        .find(|span| {
            span.name == "mf.node"
                && span.attributes.iter().any(|attribute| {
                    attribute.key.as_str() == "mf.node.id"
                        && attribute.value.as_str() == "iteration"
                })
        })
        .unwrap();
    for index in 0..3 {
        let item_record = nested
            .iter()
            .find(|record| {
                record.event_name == "mf.iteration.item.started"
                    && record.attributes["mf.iteration.index"] == index
            })
            .unwrap();
        let item_span = spans
            .iter()
            .find(|span| {
                span.span_context.span_id().to_string()
                    == item_record.trace_context.as_ref().unwrap().span_id
            })
            .unwrap();
        assert_eq!(item_span.name, "mf.iteration.item");
        assert_eq!(item_span.parent_span_id, outer_span.span_context.span_id());
        let node_record = nested
            .iter()
            .find(|record| {
                record.event_name == "mf.iteration.node.started"
                    && record.attributes["mf.iteration.index"] == index
            })
            .unwrap();
        let node_span = spans
            .iter()
            .find(|span| {
                span.span_context.span_id().to_string()
                    == node_record.trace_context.as_ref().unwrap().span_id
            })
            .unwrap();
        assert_eq!(node_span.name, "mf.iteration.node");
        assert_eq!(node_span.parent_span_id, item_span.span_context.span_id());
    }

    let definition: WorkflowDefinition =
        serde_json::from_value(self::definition(json!([1, 0, 3]), "parallel", "terminate"))
            .unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let harness = capture::Harness::new(true);
    let observation = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    let error = execute_compiled(&plan, &registry, Some(observation))
        .unwrap_err()
        .to_string();
    assert!(error.contains("iteration item 1"));
    let records = harness.records();
    assert!(records.iter().any(|record| {
        record.event_name == "mf.iteration.item.finished"
            && record.attributes["mf.iteration.index"] == 1
            && record.attributes["mf.outcome"] == "failed"
    }));
    assert!(records.iter().any(|record| {
        record.event_name == "mf.node.finished"
            && record.attributes["mf.node.id"] == "iteration"
            && record.attributes["mf.outcome"] == "failed"
    }));
}

#[test]
fn body_plugins_inherit_the_correlated_node_span() {
    let mut value = definition(json!([1, 2, 3]), "parallel", "terminate");
    value["nodes"][1]["config"]["body"] = json!({
        "nodes":[{"id":"probe","kind":"test.iteration_trace_probe"}],
        "control_edges":[{
            "from_node":"@iteration",
            "from_output":"items",
            "to_node":"probe"
        }],
        "result":{"node":"probe","port":"span"}
    });
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let harness = capture::Harness::new(true);
    let observation = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    let outputs = execute_compiled(&plan, &registry, Some(observation)).unwrap();
    let results = outputs["results"].as_array().unwrap();
    let records = harness.records();
    for (index, result) in results.iter().enumerate() {
        let started = records
            .iter()
            .find(|record| {
                record.event_name == "mf.iteration.node.started"
                    && record.attributes["mf.iteration.index"] == index
            })
            .unwrap();
        assert_eq!(
            result.as_str().unwrap(),
            started.trace_context.as_ref().unwrap().span_id
        );
    }
}

#[test]
fn item_and_body_logs_remain_live_when_traces_are_unsampled() {
    let definition: WorkflowDefinition =
        serde_json::from_value(definition(json!([1, 0]), "parallel", "continue_on_error")).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let harness = capture::Harness::new(false);
    let observation = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    execute_compiled(&plan, &registry, Some(observation)).unwrap();
    assert!(harness.spans.get_finished_spans().unwrap().is_empty());
    assert_eq!(
        harness
            .records()
            .iter()
            .filter(|record| record.scope == "mf.iteration")
            .count(),
        8
    );
}

#[test]
fn skipped_body_nodes_report_their_item_and_dependency() {
    let mut value = definition(json!([1, 2]), "sequential", "continue_on_error");
    value["nodes"][1]["config"]["body"] = json!({
        "nodes":[
            {"id":"route","kind":"builtin.if_else","config":{"branches":[{
                "id":"one",
                "condition":{
                    "source":{"output":"@iteration.items","path":""},
                    "operator":"eq",
                    "value":1
                }
            }]}},
            {"id":"copy","kind":"builtin.identity"}
        ],
        "edges":[
            {"from_node":"@iteration","from_output":"items","to_node":"copy","to_input":"input"}
        ],
        "control_edges":[
            {"from_node":"@iteration","from_output":"items","to_node":"route"},
            {"from_node":"route","from_output":"one","to_node":"copy"}
        ],
        "result":{"node":"copy","port":"value"}
    });
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let harness = capture::Harness::new(true);
    let observation = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    let result = execute_compiled(&plan, &registry, Some(observation)).unwrap();
    assert_eq!(json!(result), json!({"results":[1, null]}));
    let records = harness.records();
    let skipped = records
        .iter()
        .find(|record| record.event_name == "mf.iteration.node.skipped")
        .unwrap();
    assert_eq!(skipped.attributes["mf.iteration.index"], 1);
    assert_eq!(skipped.attributes["mf.node.id"], "copy");
    assert_eq!(
        skipped.attributes["mf.skip.causes"],
        json!([{"source_node":"route","source_output":"one"}])
    );
}

fn assert_generated_observation(project: &Path, scratch: &Path) {
    let manifest = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        format!(
            "{manifest}\nopentelemetry = {{ version = \"0.33.0\", default-features = false, features = [\"logs\", \"trace\"] }}\nopentelemetry_sdk = {{ version = \"0.33.0\", default-features = false, features = [\"logs\", \"trace\", \"testing\"] }}\n"
        ),
    )
    .unwrap();
    fs::write(
        project.join("src/observation_capture.rs"),
        include_str!("fixtures/observation_capture.rs"),
    )
    .unwrap();
    fs::write(
        project.join("src/main.rs"),
        r#"
extern crate node_0 as _;
extern crate node_1 as _;
mod workflow;
mod observation_capture;

fn main() {
    let plan = mf_compiler::CompiledWorkflow::from_json(include_str!("../workflow-plan.json")).unwrap();
    let harness = observation_capture::Harness::new(true);
    let observation = plan.start_observation(&harness.observer(), mf_telemetry::identity::RunId::new()).unwrap();
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let result = workflow::run_workflow_with_observation(&registry, Some(observation)).unwrap();
    let records = harness.records();
    let nested: Vec<_> = records.iter().filter(|record| record.scope == "mf.iteration").collect();
    let spans = harness.spans.get_finished_spans().unwrap();
    let outer = spans.iter().find(|span| span.name == "mf.node" && span.attributes.iter().any(|attribute| attribute.key.as_str() == "mf.node.id" && attribute.value.as_str() == "iteration")).unwrap();
    let items: Vec<_> = spans.iter().filter(|span| span.name == "mf.iteration.item").collect();
    let nodes: Vec<_> = spans.iter().filter(|span| span.name == "mf.iteration.node").collect();
    let correlated = items.iter().all(|span| span.parent_span_id == outer.span_context.span_id())
        && nodes.iter().all(|span| items.iter().any(|item| span.parent_span_id == item.span_context.span_id()))
        && nodes.iter().all(|span| span.span_context.trace_id() == outer.span_context.trace_id());
    println!("{}", serde_json::json!({
        "result": result,
        "nested_records": nested.len(),
        "failed_nodes": nested.iter().filter(|record| record.event_name == "mf.iteration.node.finished" && record.attributes["mf.outcome"] == "failed").count(),
        "item_spans": items.len(),
        "node_spans": nodes.len(),
        "correlated": correlated
    }));
}
"#,
    )
    .unwrap();
    mf_compiler::resolve_project(project, &scratch.join("observed.lock"), false).unwrap();
    let build = mf_compiler::cargo_command(project)
        .args(["build", "--release", "--offline", "--locked"])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let output = Command::new(common::runner_executable(project, "release"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["result"], json!({"results":[10, null, 5]}));
    assert_eq!(report["nested_records"], 12);
    assert_eq!(report["failed_nodes"], 1);
    assert_eq!(report["item_spans"], 3);
    assert_eq!(report["node_spans"], 3);
    assert_eq!(report["correlated"], true);
}

#[test]
fn generated_parallel_runner_matches_in_memory_and_describes_one_iteration_node() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("flow.json");
    let output = root.path().join("flow");
    let build = root.path().join("build");
    let mut value: Value =
        serde_json::from_str(include_str!("../../../examples/iteration.json")).unwrap();
    value["dependencies"]["core"]["path"] = json!(common::crates_dir().join("builtin-nodes/core"));
    value["dependencies"]["code"]["path"] = json!(common::crates_dir().join("builtin-nodes/code"));
    value["nodes"][1]["config"]["mode"] = json!("parallel");
    value["nodes"][1]["config"]["on_error"] = json!("continue_on_error");
    value["nodes"][0]["config"]["value"] = json!([1, 0, 2]);
    value["nodes"][1]["config"]["body"]["nodes"][0]["config"]["code"]["value"] = json!("10 / item");
    let expected = execute(value.clone()).unwrap();
    assert_eq!(expected, json!({"results":[10, null, 5]}));
    fs::write(&path, value.to_string()).unwrap();
    let support = SupportPackages::Local {
        crates_dir: common::crates_dir(),
    };
    let request = CompileRequest {
        definition: &path,
        output: &output,
        locked: false,
        build_dir: Some(&build),
        support: &support,
    };
    compile_project(&request).unwrap();
    let actual = Command::new(&output).output().unwrap();
    assert!(
        actual.status.success(),
        "{}",
        String::from_utf8_lossy(&actual.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&actual.stdout).unwrap(),
        expected
    );
    let described = Command::new(&output).arg("--describe").output().unwrap();
    assert!(described.status.success());
    let description: Value = serde_json::from_slice(&described.stdout).unwrap();
    assert_eq!(description["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(description["nodes"][1]["kind"], "builtin.iteration");
    assert_generated_observation(&build, root.path());

    value["nodes"][1]["config"]["on_error"] = json!("terminate");
    let expected = execute(value.clone()).unwrap_err();
    fs::write(&path, value.to_string()).unwrap();
    compile_project(&request).unwrap();
    let failed = Command::new(&output).output().unwrap();
    assert!(!failed.status.success());
    assert_eq!(String::from_utf8_lossy(&failed.stderr).trim(), expected);

    let installed = fs::read(&output).unwrap();
    let lock = fs::read(path.with_extension("lock")).unwrap();
    value["dependencies"]
        .as_object_mut()
        .unwrap()
        .remove("core");
    value["nodes"][0] = json!({
        "id":"source",
        "kind":"builtin.code",
        "config":{"language":"cel","inputs":{},"code":{"values":"[1, 2]"}}
    });
    value["nodes"][1]["config"]["body"] = json!({
        "nodes":[],
        "result":{"node":"@iteration","port":"items"}
    });
    value["edges"][0]["from_output"] = json!("values");
    fs::write(&path, value.to_string()).unwrap();
    let error = compile_project(&request).unwrap_err();
    assert_eq!(error.stage, "runner validation");
    let validation = Command::new(common::runner_executable(&build, "release"))
        .arg("--validate")
        .output()
        .unwrap();
    assert!(!validation.status.success());
    assert!(String::from_utf8_lossy(&validation.stderr).contains("builtin.iteration"));
    assert_eq!(fs::read(&output).unwrap(), installed);
    assert_eq!(fs::read(path.with_extension("lock")).unwrap(), lock);
}

#[test]
fn loop_and_iteration_share_a_workflow_in_memory_and_generated_runners() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("flow.json");
    let output = root.path().join("flow");
    let build = root.path().join("build");
    let mut flow: Value =
        serde_json::from_str(include_str!("../../../examples/loop.json")).unwrap();
    let iteration: Value =
        serde_json::from_str(include_str!("../../../examples/iteration.json")).unwrap();
    flow["nodes"]
        .as_array_mut()
        .unwrap()
        .extend(iteration["nodes"].as_array().unwrap().iter().cloned());
    flow["edges"]
        .as_array_mut()
        .unwrap()
        .extend(iteration["edges"].as_array().unwrap().iter().cloned());
    flow["outputs"]
        .as_array_mut()
        .unwrap()
        .extend(iteration["outputs"].as_array().unwrap().iter().cloned());
    flow["dependencies"]["core"]["path"] = json!(common::crates_dir().join("builtin-nodes/core"));
    flow["dependencies"]["code"]["path"] = json!(common::crates_dir().join("builtin-nodes/code"));

    let expected = json!({"count": 3, "results": [2, 5, 8]});
    assert_eq!(execute(flow.clone()).unwrap(), expected);
    fs::write(&path, flow.to_string()).unwrap();
    compile_project(&CompileRequest {
        definition: &path,
        output: &output,
        locked: false,
        build_dir: Some(&build),
        support: &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    })
    .unwrap();
    let result = Command::new(&output).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        expected
    );
    let described = Command::new(&output).arg("--describe").output().unwrap();
    assert!(described.status.success());
    let description: Value = serde_json::from_slice(&described.stdout).unwrap();
    assert_eq!(description["version"], "2026-09-29");
    assert_eq!(description["nodes"].as_array().unwrap().len(), 4);
    assert_eq!(description["loop_bodies"].as_array().unwrap().len(), 1);

    let mut nested: Value =
        serde_json::from_str(include_str!("../../../examples/loop.json")).unwrap();
    nested["dependencies"] = flow["dependencies"].clone();
    nested["nodes"][0]["config"]["value"] = json!([1, 2, 3]);
    nested["nodes"][1]["loop"]["max_iterations"] = json!(2);
    nested["nodes"][1]["loop"]["variables"] = json!([{"name": "items", "type": "array"}]);
    nested["nodes"][1]["loop"]["until"] = Value::Null;
    nested["nodes"][1]["loop"]["body"] = json!({
        "nodes": [
            iteration["nodes"][1].clone(),
            {"id": "assign", "kind": "workflow.loop_assign", "config": {"variable": "items"}}
        ],
        "edges": [
            {"from_node": "$loop", "from_output": "items", "to_node": "iteration", "to_input": "items"},
            {"from_node": "iteration", "from_output": "results", "to_node": "assign", "to_input": "value"}
        ]
    });
    nested["edges"][0]["to_input"] = json!("items");
    nested["outputs"][0]["name"] = json!("items");
    nested["outputs"][0]["port"] = json!("items");
    let nested_expected = json!({"items": [4, 11, 18]});
    assert_eq!(execute(nested.clone()).unwrap(), nested_expected);

    let definition: WorkflowDefinition = serde_json::from_value(nested.clone()).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let harness = capture::Harness::new(true);
    let observation = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    let observed = execute_compiled(&plan, &registry, Some(observation)).unwrap();
    assert_eq!(json!(observed), nested_expected);
    assert!(
        harness
            .records()
            .iter()
            .any(|record| record.scope == "mf.iteration")
    );

    fs::write(&path, nested.to_string()).unwrap();
    compile_project(&CompileRequest {
        definition: &path,
        output: &output,
        locked: false,
        build_dir: Some(&build),
        support: &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    })
    .unwrap();
    let nested_result = Command::new(&output).output().unwrap();
    assert!(
        nested_result.status.success(),
        "{}",
        String::from_utf8_lossy(&nested_result.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&nested_result.stdout).unwrap(),
        nested_expected
    );
}
