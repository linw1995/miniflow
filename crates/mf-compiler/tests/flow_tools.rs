//! Provider-owned catalog/dispatch sharing producer-owned native Flow values.
extern crate mfn_code as _;
extern crate mfn_core as _;
extern crate mfn_llm as _;

#[path = "fixtures/observation_capture.rs"]
mod capture;
mod common;
#[path = "../examples/flow_tools.rs"]
mod example;
#[path = "common/snapshots.rs"]
mod snapshots;
#[path = "common/tool_values.rs"]
mod tool_values;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{ExecutionContext, WorkflowArguments};
use mfn_llm::tools::ParameterDescription;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, process::Command};
use tool_values::{FlowTool, configured_agent, configured_tools};

fn body(value: Value) -> WorkflowDefinition {
    WorkflowDefinition::from_json(&value.to_string()).unwrap()
}

fn addition() -> FlowTool {
    FlowTool {
        description: "Add two integers.".into(),
        parameters: BTreeMap::from([
            (
                "left".into(),
                ParameterDescription {
                    description: "Left operand.".into(),
                },
            ),
            (
                "right".into(),
                ParameterDescription {
                    description: "Right operand.".into(),
                },
            ),
        ]),
        body: body(json!({
            "version":"2026-10-28", "nodes":[{
                "id":"calculate", "kind":"builtin.code", "config":{
                    "language":"cel", "inputs":{"left":"int", "right":"int"},
                    "code":{"sum":"left + right"}
                }
            }], "outputs":[{"name":"sum", "node":"calculate", "port":"sum"}]
        })),
    }
}

fn text() -> FlowTool {
    FlowTool {
        description: "Return text with a default.".into(),
        parameters: BTreeMap::new(),
        body: body(json!({
            "version":"2026-10-28", "nodes":[{
                "id":"params", "kind":"builtin.parameters", "config":{"parameters":{
                    "content":{"type":"string", "default":"ready", "secret":false}
                }}
            }], "outputs":[{"name":"content", "node":"params", "port":"content"}]
        })),
    }
}

fn calls() -> Value {
    json!([
        {"id":"one", "name":"add", "arguments":{"left":2,"right":3}},
        {"id":"unknown", "name":"missing", "arguments":{}},
        {"id":"bad", "name":"add", "arguments":{"left":"not an integer", "right":2}},
        {"id":"text", "name":"echo", "arguments":{}},
        {"id":"two", "name":"add", "arguments":{"left":10, "right":7}}
    ])
}

fn execute(definition: &WorkflowDefinition, calls: Value) -> Value {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(definition, &registry).unwrap();
    serde_json::to_value(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(
                WorkflowArguments::try_from(json!({"params.calls":calls})).unwrap(),
            )
            .unwrap(),
    )
    .unwrap()
}

fn assert_results(value: &Value) {
    let catalog = value["catalog"].as_array().unwrap();
    assert_eq!(catalog.len(), 2);
    assert_eq!(catalog[0]["name"], "add");
    assert_eq!(catalog[0]["description"], "Add two integers.");
    assert_eq!(
        catalog[0]["input_schema"],
        json!({
            "type":"object", "properties":{
                "left":{"type":"integer", "description":"Left operand."},
                "right":{"type":"integer", "description":"Right operand."}
            }, "required":["left","right"], "additionalProperties":false
        })
    );
    let results = value["results"].as_array().unwrap();
    assert_eq!(results.len(), 5);
    for (result, id) in results.iter().zip(["one", "unknown", "bad", "text", "two"]) {
        assert_eq!(result["tool_call_id"], id);
    }
    assert_eq!(results[0]["content"], "{\"sum\":5}");
    assert_eq!(results[3]["content"], "ready");
    assert_eq!(results[4]["content"], "{\"sum\":17}");
    for index in [1, 2] {
        let error: Value =
            serde_json::from_str(results[index]["content"].as_str().unwrap()).unwrap();
        assert!(error.get("error").is_some());
    }
}

#[test]
fn catalog_and_parallel_dispatch_use_actual_children_and_one_shared_chain() {
    let definition = configured_tools(BTreeMap::from([
        ("add".into(), addition()),
        ("echo".into(), text()),
    ]));
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    // The caller constants alone own business bodies. Native map construction
    // does not duplicate them under the catalog or dispatch callsites.
    let producers: Vec<_> = plan
        .graph
        .definition
        .nodes
        .iter()
        .filter(|node| node.kind == "builtin.constant" && node.config["type"] == "flow")
        .collect();
    assert_eq!(producers.len(), 2);
    for producer in producers {
        let children = &plan.graph.children[&producer.id];
        assert_eq!(children.len(), 1);
        assert!(children.contains_key("value"));
    }
    assert!(!plan.graph.children.contains_key(&"tools".into()));
    let empty = compile_definition(&configured_tools(BTreeMap::new()), &registry).unwrap();
    for owner in ["catalog", "dispatch"] {
        assert_eq!(
            plan.graph.children[&owner.into()],
            empty.graph.children[&owner.into()]
        );
    }
    let catalog_call = &plan.graph.children[&"catalog".into()]["body"];
    assert_eq!(
        catalog_call
            .definition
            .outputs
            .iter()
            .map(|output| output.name.as_ref())
            .collect::<Vec<_>>(),
        ["tools"]
    );
    let dispatch_call = &plan.graph.children[&"dispatch".into()]["body"];
    let params = dispatch_call
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "params")
        .unwrap();
    assert_eq!(
        params.config["parameters"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["calls", "tool_properties", "tools"]
    );
    let dispatch = &dispatch_call.children[&"dispatch".into()]["body"];
    assert!(dispatch.children.is_empty());
    assert!(
        dispatch
            .definition
            .nodes
            .iter()
            .all(|node| node.kind != "llm.tool_arguments")
    );
    assert_eq!(
        dispatch
            .definition
            .nodes
            .iter()
            .filter(|node| node.kind == "builtin.code")
            .count(),
        2
    );
    assert_eq!(
        dispatch
            .definition
            .nodes
            .iter()
            .filter(|node| node.kind == "llm.tool_result")
            .count(),
        1
    );
    assert_results(&execute(&definition, calls()));
    let bytes = plan.manifest(&registry).unwrap().to_bytes().unwrap();
    let restored = mf_runtime::WorkflowManifest::from_bytes(&bytes).unwrap();
    assert_eq!(restored.graph, plan.graph);
    assert_eq!(
        restored.description().unwrap(),
        mf_compiler::describe_compiled(&plan).unwrap()
    );
    let roundtrip =
        mf_compiler::CompiledWorkflow::from_json(&serde_json::to_string(&plan).unwrap()).unwrap();
    assert_eq!(roundtrip, plan);
}

#[test]
fn configured_example_executes_and_exports_an_ordinary_workflow() {
    let definition = configured_tools(BTreeMap::from([
        ("add".into(), addition()),
        ("echo".into(), text()),
    ]));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tools.json");
    fs::write(&path, serde_json::to_vec_pretty(&definition).unwrap()).unwrap();
    let exported = WorkflowDefinition::from_json(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(exported, definition);
    assert_results(&execute(&exported, calls()));
    assert_eq!(
        exported
            .nodes
            .iter()
            .filter(|node| node.kind == "workflow.call")
            .count(),
        2
    );
    assert!(
        !exported
            .nodes
            .iter()
            .any(|node| node.kind == mf_runtime::ITERATION_KIND)
    );
    let tools = exported
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "tools")
        .unwrap();
    assert_eq!(tools.kind, "builtin.object");
    assert_eq!(tools.config, json!({"fields":{"add":"flow","echo":"flow"}}));
    let properties = exported
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "tool_add_properties")
        .unwrap();
    assert_eq!(properties.kind, "builtin.object");
    assert_eq!(
        properties.config,
        json!({"fields":{"description":"string","parameters":"object","interface":"object"}})
    );
    let description = exported
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "tool_add_description")
        .unwrap();
    assert_eq!(description.config["value"], "Add two integers.");
    assert!(exported.edges.iter().any(|edge| {
        edge.from_node.as_str() == "tool_add_description"
            && edge.from_output == "value"
            && edge.to_node.as_str() == "tool_add_properties"
            && edge.to_input == "description"
    }));
    assert!(
        !exported
            .nodes
            .iter()
            .any(|node| node.id.as_str() == "descriptions")
    );
}

#[test]
fn fixed_agent_builtin_owns_no_caller_tool_bodies_or_per_tool_structure() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition = configured_agent(BTreeMap::from([
        ("add".into(), addition()),
        ("echo".into(), text()),
    ]));
    let plan = compile_definition(&definition, &registry).unwrap();
    let empty = compile_definition(&configured_agent(BTreeMap::new()), &registry).unwrap();
    assert_eq!(
        plan.graph.children[&"agent".into()],
        empty.graph.children[&"agent".into()]
    );
    assert!(!plan.graph.children.contains_key(&"tools".into()));
    let producers: Vec<_> = plan
        .graph
        .definition
        .nodes
        .iter()
        .filter(|node| node.kind == "builtin.constant" && node.config["type"] == "flow")
        .collect();
    assert_eq!(producers.len(), 2);
    for producer in producers {
        assert_eq!(plan.graph.children[&producer.id].len(), 1);
        assert!(plan.graph.children[&producer.id].contains_key("value"));
    }
    let agent = definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "agent")
        .unwrap();
    assert_eq!(agent.kind, "workflow.call");
    assert_eq!(agent.config, json!({"flow":"agent"}));
    assert_eq!(definition.outputs.len(), 5);
}

#[test]
fn empty_catalog_and_dispatch_keep_capabilities_internal() {
    use mf_runtime::{TypeCompatibility, ValueType};
    // builtin.object with no fields has Map<Any>, accepted through the normal
    // checked connection to the provider's Map<Object> parameter, not a JSON shim.
    assert_eq!(
        ValueType::Map(Box::new(ValueType::Any))
            .compatibility_with(&ValueType::Map(Box::new(ValueType::Object))),
        TypeCompatibility::Checked
    );
    let definition = configured_tools(BTreeMap::new());
    let tools = definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "tools")
        .unwrap();
    assert_eq!(tools.config, json!({"fields":{}}));
    assert_eq!(
        execute(&definition, json!([])),
        json!({"catalog":[], "results":[]})
    );
    let result = execute(
        &definition,
        json!([{"id":"unknown", "name":"missing", "arguments":{}}]),
    );
    assert_eq!(result["catalog"], json!([]));
    assert!(
        result["results"][0]["content"]
            .as_str()
            .unwrap()
            .contains("unknown tool")
    );
    let value = serde_json::to_value(definition).unwrap();
    assert!(value.get("flow_arguments").is_none());
    assert!(!value.to_string().contains("compiler.metadata_expansion"));
}

#[test]
fn every_tool_interface_is_validated_even_without_calls() {
    let mut tool = addition();
    tool.parameters.insert(
        "missing".into(),
        ParameterDescription {
            description: "Not an input.".into(),
        },
    );
    let definition = configured_tools(BTreeMap::from([("add".into(), tool)]));
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    for calls in [
        json!([]),
        json!([{"id":"valid","name":"add","arguments":{"left":2,"right":3}}]),
    ] {
        let recording = snapshots::Snapshots::memory();
        let mut context = ExecutionContext::default();
        context.set_snapshot_recorder(recording.recorder());
        context.set_workflow_arguments(
            WorkflowArguments::try_from(json!({"params.calls":calls})).unwrap(),
        );
        let error = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_in_context(&mut context)
            .unwrap_err();
        assert!(
            error.to_string().contains("declared inputs only"),
            "{error}"
        );
        assert!(std::error::Error::source(&error).is_some());
        // Even a valid business call cannot start before catalog validation.
        assert!(!recording.history().iter().any(|entry| {
            matches!(entry.node.as_ref(), "dispatch" | "calculate")
                && entry
                    .snapshot
                    .node(&entry.scope, &entry.node)
                    .unwrap()
                    .outcome
                    == mf_runtime::SnapshotOutcome::Started
        }));
    }
}

#[test]
fn runtime_only_tool_inputs_are_rejected_before_dispatch() {
    for value_type in [json!("flow"), json!({"list":"flow"}), json!({"map":"flow"})] {
        let tool = FlowTool {
            description: "A model cannot provide callable capabilities.".into(),
            parameters: BTreeMap::new(),
            body: body(json!({"version":"2026-10-28", "nodes":[
                {"id":"params", "kind":"builtin.parameters", "config":{"parameters":{
                    "policy":{"type":value_type, "secret":false}
                }}}
            ], "outputs":[{"name":"policy", "node":"params", "port":"policy"}]})),
        };
        let definition = configured_tools(BTreeMap::from([("native".into(), tool)]));
        let registry = NodeRegistry::from_inventory().unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let error = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(WorkflowArguments::try_from(json!({"params.calls":[]})).unwrap())
            .unwrap_err();
        assert!(std::error::Error::source(&error).is_some());
        assert!(error.to_string().contains("runtime-only"), "{error}");
    }
}

static TOOL_EXECUTIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[derive(mf_runtime::NodeValue)]
struct EffectInput {
    value: i64,
}

#[derive(mf_runtime::NodeValue)]
struct EffectOutput {
    content: String,
}

struct Effect;
impl mf_runtime::TypedTaskNode for Effect {
    type Input = EffectInput;
    type Output = EffectOutput;

    fn execute(
        &self,
        input: EffectInput,
        _: &mut ExecutionContext,
    ) -> Result<mf_runtime::TypedNodeResult<EffectOutput>, mf_runtime::NodeExecutionError> {
        TOOL_EXECUTIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(EffectOutput {
            content: input.value.to_string(),
        }
        .into())
    }
}

inventory::submit! {
    mf_runtime::NodeRegistration {
        kind: "test.flow_tool_effect",
        factory: mf_runtime::NodeFactory::Plain(|_| {
            mf_runtime::PreparedNode::typed_task(Effect, mf_runtime::NodeMetadata::default())
        }),
    }
}

#[test]
fn unknown_and_invalid_calls_do_not_execute_tool_business_work() {
    let tool = FlowTool {
        description: "Execute only after argument validation.".into(),
        parameters: BTreeMap::new(),
        body: body(json!({"version":"2026-10-28", "nodes":[
            {"id":"effect", "kind":"test.flow_tool_effect"}
        ], "outputs":[{"name":"content", "node":"effect", "port":"content"}]})),
    };
    let definition = configured_tools(BTreeMap::from([("effect".into(), tool)]));
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    assert_eq!(TOOL_EXECUTIONS.load(std::sync::atomic::Ordering::SeqCst), 0);
    let output = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_with_inputs(
            WorkflowArguments::try_from(json!({"params.calls":[
                {"id":"unknown", "name":"missing", "arguments":{"value":1}},
                {"id":"invalid", "name":"effect", "arguments":{"value":"wrong"}},
                {"id":"valid", "name":"effect", "arguments":{"value":42}}
            ]}))
            .unwrap(),
        )
        .unwrap();
    assert_eq!(TOOL_EXECUTIONS.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(output["results"][2]["content"], json!("42"));
    for index in [0, 1] {
        assert!(
            output["results"][index]["content"]
                .as_str()
                .unwrap()
                .contains("error")
        );
    }
}

static BATCH_EFFECTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct BatchEffect;
impl mf_runtime::TypedTaskNode for BatchEffect {
    type Input = EffectInput;
    type Output = EffectOutput;
    fn execute(
        &self,
        input: EffectInput,
        _: &mut ExecutionContext,
    ) -> Result<mf_runtime::TypedNodeResult<EffectOutput>, mf_runtime::NodeExecutionError> {
        BATCH_EFFECTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(EffectOutput {
            content: input.value.to_string(),
        }
        .into())
    }
}
inventory::submit! {
    mf_runtime::NodeRegistration {
        kind:"test.flow_tool_batch_effect",
        factory:mf_runtime::NodeFactory::Plain(|_| mf_runtime::PreparedNode::typed_task(BatchEffect,mf_runtime::NodeMetadata::default())),
    }
}

#[test]
fn malformed_call_batches_fail_before_any_tool_effects() {
    let tool = FlowTool {
        description: "Validate every call first.".into(),
        parameters: BTreeMap::new(),
        body: body(
            json!({"version":"2026-10-28","nodes":[{"id":"effect","kind":"test.flow_tool_batch_effect"}],"outputs":[{"name":"content","node":"effect","port":"content"}]}),
        ),
    };
    let definition = configured_tools(BTreeMap::from([("effect".into(), tool)]));
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    for malformed in [
        json!({"id":42,"name":"missing","arguments":{}}),
        json!({"id":" \t\n","name":"missing","arguments":{}}),
        json!({"id":"\u{2003}\u{00a0}","name":"missing","arguments":{}}),
        json!({"id":"missing-arguments","name":"missing"}),
        json!({"id":"extra","name":"missing","arguments":{},"unexpected":true}),
    ] {
        let args=WorkflowArguments::try_from(json!({"params.calls":[{"id":"valid","name":"effect","arguments":{"value":42}},malformed]})).unwrap();
        assert!(
            instantiate_compiled(&plan, &registry)
                .unwrap()
                .execute_with_inputs(args)
                .is_err()
        );
        assert_eq!(BATCH_EFFECTS.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

#[test]
fn private_arguments_do_not_hide_other_calls_or_public_selections() {
    use mf_runtime::ValueRef;
    let definition = configured_tools(BTreeMap::from([("echo".into(), text())]));
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let private = ValueRef::object([
        ("id".into(), "private".into()),
        ("name".into(), "echo".into()),
        (
            "arguments".into(),
            ValueRef::object([(
                "content".into(),
                ValueRef::from("private text").into_secret(),
            )]),
        ),
    ]);
    let public = ValueRef::from(json!({"id":"public","name":"echo","arguments":{}}));
    let recording = snapshots::Snapshots::memory();
    let mut context = ExecutionContext::default();
    context.set_snapshot_recorder(recording.recorder());
    context.set_workflow_arguments(WorkflowArguments(BTreeMap::from([(
        "params.calls".into(),
        ValueRef::array(vec![private, public]),
    )])));
    let output = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_in_context(&mut context)
        .unwrap();
    assert!(output["results"][0].contains_secret());
    assert!(!output["results"][1].contains_secret());
    assert_eq!(output["results"][1]["content"].as_str(), Some("ready"));
    let history = recording.history();
    let names: Vec<_> = history
        .iter()
        .filter(|entry| entry.node.as_ref() == "classify")
        .filter_map(|entry| entry.snapshot.node(&entry.scope, &entry.node))
        .filter_map(|snapshot| snapshot.outputs.get("name"))
        .collect();
    // The adapter marks every derived output with its call's privacy, so the
    // private call's tool name is redacted while the public call stays readable.
    let mut names: Vec<_> = names.iter().map(|name| name.as_str()).collect();
    names.sort();
    assert_eq!(names, [Some("[REDACTED]"), Some("echo")]);
}

#[test]
fn secret_tool_results_keep_business_values_while_all_downstream_snapshots_are_redacted() {
    const SECRET: &str = "synthetic-secret-tool-output";
    for output_name in ["content", "answer"] {
        let tool = FlowTool {
            description: "Return a privately bound value.".into(),
            parameters: BTreeMap::new(),
            body: body(json!({
                "version":"2026-10-28", "nodes":[{
                    "id":"params", "kind":"builtin.parameters", "config":{"parameters":{
                        "content":{"type":"string", "default":SECRET, "secret":true}
                    }}
                }], "outputs":[{"name":output_name, "node":"params", "port":"content"}]
            })),
        };
        let definition = configured_tools(BTreeMap::from([("private".into(), tool)]));
        let registry = NodeRegistry::from_inventory().unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let recording = snapshots::Snapshots::memory();
        let mut context = ExecutionContext::default();
        context.set_snapshot_recorder(recording.recorder());
        context.set_workflow_arguments(
            WorkflowArguments::try_from(json!({"params.calls":[
                {"id":"private", "name":"private", "arguments":{}}
            ]}))
            .unwrap(),
        );
        let outputs = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_in_context(&mut context)
            .unwrap();
        let expected = if output_name == "content" {
            SECRET.into()
        } else {
            json!({"answer":SECRET}).to_string()
        };
        assert_eq!(
            outputs["results"][0]["content"].as_str(),
            Some(expected.as_str())
        );
        assert!(outputs["results"][0].contains_secret());
        assert!(
            !serde_json::to_string(&outputs["catalog"])
                .unwrap()
                .contains(SECRET)
        );
        let history = recording.history();
        assert!(history.iter().any(|entry| entry.node.as_ref() == "encode"));
        assert!(
            history
                .iter()
                .any(|entry| entry.node.as_ref() == "dispatch")
        );
        for entry in history {
            let snapshot = entry.snapshot.node(&entry.scope, &entry.node).unwrap();
            let serialized = serde_json::to_string(snapshot).unwrap();
            assert!(
                !serialized.contains(SECRET),
                "snapshot leaked at {}: {serialized}",
                entry.node
            );
        }
    }
}

#[test]
fn secret_tool_selection_stays_private_through_native_dispatch() {
    use mf_runtime::ValueRef;
    let definition = configured_tools(BTreeMap::from([("echo".into(), text())]));
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    for name in ["echo", "missing"] {
        let call = ValueRef::object([
            ("id".into(), "selected".into()),
            ("name".into(), ValueRef::from(name).into_secret()),
            ("arguments".into(), json!({}).into()),
        ]);
        let recording = snapshots::Snapshots::memory();
        let mut context = ExecutionContext::default();
        context.set_snapshot_recorder(recording.recorder());
        context.set_workflow_arguments(WorkflowArguments(BTreeMap::from([(
            "params.calls".into(),
            ValueRef::array(vec![call]),
        )])));
        let output = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_in_context(&mut context)
            .unwrap();
        assert!(!output["catalog"].contains_secret());
        assert!(output["results"][0].contains_secret());
        let history = recording.history();
        let selected: Vec<_> = history
            .iter()
            .filter(|entry| entry.node.as_ref() == "classify")
            .filter_map(|entry| entry.snapshot.node(&entry.scope, &entry.node))
            .filter(|snapshot| snapshot.outputs.get("name").is_some())
            .collect();
        assert!(!selected.is_empty());
        for snapshot in selected {
            assert_eq!(snapshot.outputs["name"].as_str(), Some("[REDACTED]"));
        }
    }
}

#[test]
fn catalog_projects_once_before_dispatch_on_each_invocation_with_native_flows() {
    let definition = configured_tools(BTreeMap::from([("add".into(), addition())]));
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let graph = mf_compiler::describe_compiled(&plan).unwrap();
    let producer = plan
        .graph
        .definition
        .nodes
        .iter()
        .find(|node| node.kind == "builtin.constant" && node.config["type"] == "flow")
        .unwrap();
    let producer_key =
        plan.graph.children[&producer.id]["value"].child_scope_key(producer, "value");
    let call = plan
        .graph
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "dispatch")
        .unwrap();
    let dispatch_call = &plan.graph.children[&call.id]["body"];
    let iteration = dispatch_call
        .definition
        .nodes
        .iter()
        .find(|node| node.kind == mf_runtime::ITERATION_KIND)
        .unwrap();
    let iteration_path = vec![
        dispatch_call.child_scope_key(call, "body"),
        iteration.id.to_string(),
    ];
    for _ in 0..2 {
        use mf_telemetry::{event::Event, identity::RunId};
        let harness = capture::Harness::new(true);
        let observation = harness
            .observer()
            .start_with_description(graph.clone(), RunId::new())
            .unwrap();
        let output = ExecutionContext::run(Some(observation), |context| {
            context.set_workflow_arguments(
                WorkflowArguments::try_from(json!({"params.calls":calls()})).unwrap(),
            );
            flow.execute_in_context(context)
        })
        .unwrap();
        assert_eq!(output.len(), 2);
        assert_eq!(output["catalog"].as_array().unwrap().len(), 1);
        harness.traces.force_flush().unwrap();
        let spans = harness.spans.get_finished_spans().unwrap();
        // One catalog projection node runs once, outside the dispatch iteration.
        let projections = spans
            .iter()
            .filter(|span| {
                let mut node = String::new();
                let mut kind = String::new();
                for attribute in &span.attributes {
                    let text = attribute.value.as_str().to_string();
                    match attribute.key.as_str() {
                        "mf.node.id" => node = text,
                        "mf.node.kind" => kind = text,
                        _ => {}
                    }
                }
                node == "definition" && kind == "builtin.code"
            })
            .count();
        assert_eq!(projections, 1);
        // A dynamic invocation belongs to its producer's definition, while its
        // runtime ancestry remains inside the dispatch item. The extended detail
        // protocol must preserve both, rather than invent Router-owned children.
        let wire = harness.records();
        let details: Vec<_> = wire
            .iter()
            .filter_map(|record| {
                record
                    .attributes
                    .get(mf_telemetry::iteration::DETAIL_CONTEXT_ATTRIBUTE)
                    .map(|value| {
                        mf_telemetry::iteration::IterationDetailContext::from_json(
                            value.as_str().unwrap(),
                        )
                        .unwrap()
                    })
            })
            .collect();
        assert!(!details.is_empty());
        let executions: Vec<_> = details
            .iter()
            .filter(|detail| {
                detail.body_scope.iter().any(|entry| matches!(entry,
                mf_telemetry::event::ScopePathEntry::Flow { call_id, .. } if call_id == "execute"
            ))
            })
            .collect();
        assert!(!executions.is_empty());
        for detail in executions {
            assert_eq!(detail.context.iteration_path, iteration_path);
            assert!(detail.body_scope.iter().any(|entry| matches!(entry,
                mf_telemetry::event::ScopePathEntry::Flow { call_id, definition }
                if call_id == "execute" && definition.workflow_id == graph.workflow_id
                    && definition.path == vec![producer_key.clone()]
            )));
        }
        let records: Vec<_> = wire
            .into_iter()
            .filter(|record| record.scope == mf_telemetry::INSTRUMENTATION_SCOPE)
            .map(|record| record.decode().unwrap())
            .collect();
        for record in &records {
            record.validate_for(&graph).unwrap();
        }
        let catalog_done = records.iter().position(|record| matches!(
            &record.event, Event::NodeFinished {node, ..} if node.id == "catalog" && node.path.is_empty()
        )).unwrap();
        let dispatch_start = records.iter().position(|record| matches!(
            &record.event, Event::NodeStarted {node, ..} if node.id == "dispatch" && node.path.is_empty()
        )).unwrap();
        assert!(catalog_done < dispatch_start);
    }
    assert!(plan.graph.definition.control_edges.iter().any(|edge| {
        edge.from_node.as_str() == "catalog"
            && edge.from_output == "tools"
            && edge.to_node.as_str() == "dispatch"
    }));
}

#[cfg(feature = "codegen")]
#[test]
fn source_free_tool_catalog_and_dispatch_runner_matches_memory() {
    use mf_compiler::{CompileRequest, SupportPackages, compile_project};
    let definition = configured_tools(BTreeMap::from([
        ("add".into(), addition()),
        ("echo".into(), text()),
    ]));
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("tools.json");
    fs::write(&source, serde_json::to_vec(&definition).unwrap()).unwrap();
    let executable = root.path().join("runner");
    let build = root.path().join("build");
    compile_project(&CompileRequest {
        definition: &source,
        output: &executable,
        locked: false,
        build_dir: Some(&build),
        support: &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    })
    .unwrap();
    fs::remove_file(source).unwrap();
    fs::remove_dir_all(build).unwrap();
    let result = Command::new(&executable)
        .current_dir(root.path())
        .env("PATH", "")
        .arg(format!("--params.calls={}", calls()))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_results(&value);
    assert_eq!(value, execute(&definition, calls()));
}
