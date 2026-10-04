mod common;

extern crate mfn_core as _;
use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, describe_workflow_inputs,
    instantiate_compiled, plan_definition,
};
use mf_runtime::{
    ExecutionContext, InputResource, Inputs, NodeBuildError, NodeExecutionError, NodeFactory,
    NodeMetadata, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, PreparedNode,
    TaskNode, ValueType, WorkflowArguments, WorkflowInput,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};

static EXECUTIONS: AtomicUsize = AtomicUsize::new(0);

struct Echo;
impl TaskNode for Echo {
    fn execute(
        &self,
        inputs: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        EXECUTIONS.fetch_add(1, Ordering::SeqCst);
        Ok(Outputs::from([(
            "value".into(),
            mf_runtime::ValueRef::object(
                inputs.into_iter().map(|(name, value)| (name.into(), value)),
            ),
        )])
        .into())
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let inputs: BTreeMap<String, WorkflowInput> =
        mf_runtime::deserialize_config(config["inputs"].clone())?;
    let resources = mf_runtime::deserialize_config(
        config
            .get("resources")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )?;
    Ok(PreparedNode::new(
        Echo,
        NodeMetadata {
            resources,
            ..NodeMetadata::new(NodePorts {
                inputs: inputs
                    .into_iter()
                    .map(|(name, port)| PortSpec::owned(name, port.value_type, port.required))
                    .collect(),
                outputs: vec![PortSpec::new("value", ValueType::Object, true)],
            })
        },
    ))
}

inventory::submit! { NodeRegistration { kind: "test.startup_echo", factory: NodeFactory::Plain(factory) } }

struct Source;
impl mf_runtime::StreamNode for Source {
    fn execute(
        &mut self,
        _: Inputs,
        _: &mut ExecutionContext,
        _: &mut mf_runtime::Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        panic!("interface inspection must not execute sources");
    }
}
inventory::submit! { NodeRegistration { kind: "test.startup_source", factory: NodeFactory::Plain(|config| {
    let prepared = factory(config)?;
    Ok(PreparedNode::stream(Source, prepared.metadata))
}) } }

fn definition(nodes: Value) -> WorkflowDefinition {
    WorkflowDefinition::from_json(
        &json!({"version":"2026-10-03", "dependencies":{}, "nodes":nodes}).to_string(),
    )
    .unwrap()
}

fn echo(id: &str, inputs: Value) -> Value {
    json!({"id":id, "kind":"test.startup_echo", "config":{"inputs":inputs}})
}

#[test]
fn validates_all_roots_before_execution_and_preserves_optional_and_nested_values() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut definition = definition(json!([
        echo(
            "a",
            json!({"data":{"type":{"list":{"map":"int"}}, "required":true}, "optional":{"type":"string", "required":false}})
        ),
        echo("b./~", json!({"x./~":{"type":"any", "required":true}}))
    ]));
    definition.outputs = serde_json::from_value(json!([
        {"name":"a", "node":"a", "port":"value"},
        {"name":"b", "node":"b./~", "port":"value"}
    ]))
    .unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let before = EXECUTIONS.load(Ordering::SeqCst);
    for (value, diagnostic) in [
        (json!({"a":{"data":[]}}), "/b.~1~0/x.~1~0"),
        (
            json!({"a":{"data":[{"count":"wrong"}]}, "b./~":{"x./~":null}}),
            "/a/data/0/count",
        ),
        (
            json!({"a":{"data":[], "optional":null}, "b./~":{"x./~":null}}),
            "/a/optional",
        ),
        (
            json!({"a":{"data":[], "typo":1}, "b./~":{"x./~":null}}),
            "/a/typo",
        ),
        (json!({"unknown":{}}), "/unknown"),
    ] {
        let error = flow
            .execute_with_inputs(WorkflowArguments::try_from(value).unwrap())
            .unwrap_err();
        assert!(error.to_string().contains(diagnostic), "{error}");
        assert_eq!(EXECUTIONS.load(Ordering::SeqCst), before);
    }
    for count in [1, 2] {
        let arguments = WorkflowArguments::try_from(
            json!({"a":{"data":[{"count":count}]}, "b./~":{"x./~":null}}),
        )
        .unwrap();
        let outputs = flow.execute_with_inputs(arguments).unwrap();
        assert_eq!(outputs["a"], json!({"data":[{"count":count}]}));
        assert_eq!(outputs["b"], json!({"x./~":null}));
    }
}

#[test]
fn rejects_duplicate_keys_invalid_shapes_and_excessive_json_before_binding() {
    for bytes in [
        br#"{"a":{},"a":{}}"#.as_slice(),
        br#"{"a":{"x":1,"x":2}}"#,
        br#"{"a":{"x":{"nested":1,"nested":2}}}"#,
    ] {
        assert!(
            WorkflowArguments::from_json(bytes)
                .unwrap_err()
                .to_string()
                .contains("duplicate object key")
        );
    }
    for value in [json!(null), json!([]), json!({"a":null}), json!({"a":[]} )] {
        assert!(WorkflowArguments::try_from(value).is_err());
    }
    assert!(
        WorkflowArguments::from_json(&vec![b' '; mf_runtime::MAX_WORKFLOW_INPUT_BYTES + 1])
            .is_err()
    );
    let values = WorkflowArguments::from_json(br#"{"a":{"x":18446744073709551615}}"#).unwrap();
    assert_eq!(values.0["a"]["x"], json!(u64::MAX));
}

#[test]
fn preserves_noninitial_and_body_required_inputs_and_legacy_validation() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut definition = definition(json!([
        {"id":"a", "kind":"builtin.constant", "config":{"value":true}},
        {"id":"b", "kind":"builtin.identity"}
    ]));
    definition.control_edges =
        serde_json::from_value(json!([{"from_node":"a", "from_output":"value", "to_node":"b"}]))
            .unwrap();
    assert!(
        compile_definition(&definition, &registry)
            .unwrap_err()
            .to_string()
            .contains("requires input")
    );
    definition.control_edges.clear();
    let plan = compile_definition(&definition, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    assert!(flow.input_schema().inputs.contains_key("b"));
    let flow = flow
        .with_control_edges(
            serde_json::from_value(json!([
                {"from_node":"a", "from_output":"value", "to_node":"b"}
            ]))
            .unwrap(),
        )
        .unwrap();
    assert!(!flow.input_schema().inputs.contains_key("b"));
    definition.version = mf_runtime::WorkflowDefinitionVersion::V2026_10_02;
    assert!(compile_definition(&definition, &registry).is_err());

    let nested = definition_from_json(json!({"version":"2026-10-03", "dependencies":{}, "nodes":[
        {"id":"repeat", "kind":"workflow.loop", "loop":{"max_iterations":1, "variables":[{"name":"x", "type":"int"}], "body":{
            "nodes":[{"id":"missing", "kind":"builtin.identity"}]
        }}}
    ]}));
    assert!(
        compile_definition(&nested, &registry)
            .unwrap_err()
            .to_string()
            .contains("missing")
    );
    let mut nested = nested;
    let body = &mut nested.nodes[0].loop_definition.as_mut().unwrap().body;
    body.nodes = serde_json::from_value(json!([
        {"id":"repeat", "kind":"builtin.identity"}
    ]))
    .unwrap();
    body.edges = serde_json::from_value(json!([
        {"from_node":"%loop", "from_output":"x", "to_node":"repeat", "to_input":"input"}
    ]))
    .unwrap();
    nested.outputs = serde_json::from_value(json!([
        {"name":"value", "node":"repeat", "port":"x"}
    ]))
    .unwrap();
    let plan = compile_definition(&nested, &registry).unwrap();
    let output = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_with_inputs(WorkflowArguments::try_from(json!({"repeat":{"x":7}})).unwrap())
        .unwrap();
    assert_eq!(output["value"], json!(7));
}

fn definition_from_json(value: Value) -> WorkflowDefinition {
    WorkflowDefinition::from_json(&value.to_string()).unwrap()
}

#[test]
fn describes_dynamic_source_inputs_and_exclusive_resources_without_execution() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut definition = definition_from_json(
        json!({"version":"2026-10-03", "dependencies":{}, "nodes":[
            {"id":"source", "kind":"test.startup_echo", "config":{"inputs":{"path":{"type":"string", "required":true}}, "resources":["stdin"]}}
        ]}),
    );
    let schema =
        describe_workflow_inputs(&plan_definition(&definition).unwrap(), &registry).unwrap();
    assert_eq!(
        schema.inputs["source"]["path"].value_type,
        ValueType::String
    );
    assert_eq!(schema.resources["source"], [InputResource::Stdin]);
    assert!(
        schema
            .validate_resources(&WorkflowArguments::default(), false)
            .is_err()
    );
    schema
        .validate_resources(&WorkflowArguments::default(), true)
        .unwrap();
    let mut other = definition.nodes[0].clone();
    other.id = "other".into();
    definition.nodes.push(other);
    let error =
        describe_workflow_inputs(&plan_definition(&definition).unwrap(), &registry).unwrap_err();
    assert!(error.to_string().contains("stdin is already required"));
}

#[test]
fn source_contracts_use_ports_and_node_ids_without_special_input_names() {
    let prepared = factory(json!({"inputs":{"path":{"type":"string", "required":true}}})).unwrap();
    let source =
        mf_runtime::FlowNode::new("%input", PreparedNode::stream(Source, prepared.metadata));
    let schema =
        mf_runtime::WorkflowInputSchema::from_nodes([(&source, true)], |_, _| false).unwrap();
    schema
        .validate(&WorkflowArguments::try_from(json!({"%input":{"path":"/missing/file"}})).unwrap())
        .unwrap();
    assert_eq!(
        schema.inputs["%input"]["path"].value_type,
        ValueType::String
    );
}

#[test]
fn generated_tasks_share_startup_binding_and_validation() {
    use mf_compiler::{RunnerOptions, SupportPackages, write_dependency_project_with_options};
    use std::{fs, process::Command};
    let registry = NodeRegistry::from_inventory().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("build");
    let definition = definition_from_json(
        json!({"version":"2026-10-03", "dependencies":{"core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")}},
        "nodes":[{"id":"copy", "kind":"builtin.identity"}], "outputs":[{"name":"value", "node":"copy", "port":"value"}]}),
    );
    let plan = compile_definition(&definition, &registry).unwrap();
    write_dependency_project_with_options(
        &project,
        &plan,
        &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
        &RunnerOptions { telemetry: false },
    )
    .unwrap();
    fs::write(project.join("src/main.rs"), r#"extern crate node_0 as _;
mod workflow;
fn main() {
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let arguments = mf_runtime::WorkflowArguments::from_json(std::env::args().nth(1).unwrap().as_bytes()).unwrap();
    match workflow::run_workflow_with_inputs(&registry, arguments) {
        Ok(outputs) => println!("{}", serde_json::to_string(&outputs).unwrap()),
        Err(error) => { eprintln!("{error}"); std::process::exit(1); }
    }
}
"#).unwrap();
    let build = mf_compiler::cargo_command(&project)
        .args(["build", "--offline", "--release"])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    for value in [json!({"copy":{"input":{"items":[1,2]}}}), json!({})] {
        let actual = Command::new(common::runner_executable(&project, "release"))
            .arg(value.to_string())
            .output()
            .unwrap();
        let expected = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(WorkflowArguments::try_from(value).unwrap());
        match expected {
            Ok(outputs) => {
                assert!(
                    actual.status.success(),
                    "{}",
                    String::from_utf8_lossy(&actual.stderr)
                );
                assert_eq!(
                    serde_json::from_slice::<Value>(&actual.stdout).unwrap(),
                    serde_json::to_value(outputs).unwrap()
                );
            }
            Err(error) => {
                assert!(!actual.status.success());
                assert!(String::from_utf8_lossy(&actual.stderr).contains(&error.to_string()));
            }
        }
    }
}
