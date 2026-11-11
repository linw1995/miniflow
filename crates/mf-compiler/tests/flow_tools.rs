//! Provider-owned catalog/dispatch sharing producer-owned native Flow values.
extern crate mfn_code as _;
extern crate mfn_core as _;
extern crate mfn_llm as _;

#[path = "fixtures/observation_capture.rs"]
mod capture;
mod common;
#[path = "common/snapshots.rs"]
mod snapshots;
// Exercise the documented host/export example in the instrumented integration
// binary too, retaining its real source mapping for incremental coverage.
#[path = "../examples/flow_tools.rs"]
mod example;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{ExecutionContext, WorkflowArguments};
use mfn_llm::{
    flows::{FlowTool, tool_uses_definition},
    tools::ParameterDescription,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, process::Command};

fn body(value: Value) -> WorkflowDefinition {
    WorkflowDefinition::from_json(&value.to_string()).unwrap()
}

fn addition() -> FlowTool {
    FlowTool {
        description: "Add two integers.".into(),
        parameters: BTreeMap::from([
            (
                "calculate.left".into(),
                ParameterDescription {
                    description: "Left operand.".into(),
                },
            ),
            (
                "calculate.right".into(),
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

fn configured(tools: BTreeMap<String, FlowTool>) -> WorkflowDefinition {
    let mut definition = tool_uses_definition(tools).unwrap();
    definition.dependencies = serde_json::from_value(json!({
        "core":{"package":"mfn-core", "path": common::crates_dir().join("builtin-nodes/core")},
        "code":{"package":"mfn-code", "path": common::crates_dir().join("builtin-nodes/code")},
        "llm":{"package":"mfn-llm", "path": common::crates_dir().join("builtin-nodes/llm")}
    }))
    .unwrap();
    definition
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
    let definition = configured(BTreeMap::from([
        ("add".into(), addition()),
        ("echo".into(), text()),
    ]));
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    // Each body exists only at its producer, not again under the collector or
    // dispatch callsite. Execution retains that producer definition origin.
    assert_eq!(plan.graph.children[&"tool_0".into()].len(), 1);
    assert!(plan.graph.children[&"tool_0".into()].contains_key("body"));
    assert!(plan.graph.children[&"tool_1".into()].contains_key("body"));
    assert!(!plan.graph.children.contains_key(&"bindings".into()));
    let dispatch = &plan.graph.children[&"dispatch".into()]["body"];
    assert!(dispatch.children.is_empty());
    assert_eq!(
        dispatch
            .definition
            .nodes
            .iter()
            .filter(|node| node.kind == "llm.tool_arguments")
            .count(),
        1
    );
    assert_eq!(
        dispatch
            .definition
            .nodes
            .iter()
            .filter(|node| node.kind == "llm.tool_result")
            .count(),
        2
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
fn empty_catalog_and_dispatch_keep_capabilities_internal() {
    let definition = configured(BTreeMap::new());
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
        "calculate.missing".into(),
        ParameterDescription {
            description: "Not an input.".into(),
        },
    );
    let definition = configured(BTreeMap::from([("add".into(), tool)]));
    let registry = NodeRegistry::from_inventory().unwrap();
    let error = compile_definition(&definition, &registry).unwrap_err();
    assert!(error.to_string().contains("unknown input"), "{error}");
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn runtime_only_tool_inputs_are_rejected_during_preparation() {
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
        let definition = configured(BTreeMap::from([("native".into(), tool)]));
        let registry = NodeRegistry::from_inventory().unwrap();
        let error = compile_definition(&definition, &registry).unwrap_err();
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
    let definition = configured(BTreeMap::from([("effect".into(), tool)]));
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
        let definition = configured(BTreeMap::from([("private".into(), tool)]));
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
    let definition = configured(BTreeMap::from([("echo".into(), text())]));
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
            .filter(|entry| entry.node.as_ref() == "select")
            .filter_map(|entry| entry.snapshot.node(&entry.scope, &entry.node))
            .filter(|snapshot| snapshot.outputs.get("binding").is_some())
            .collect();
        assert!(!selected.is_empty());
        for snapshot in selected {
            for port in ["binding", "name", "route"] {
                assert_eq!(snapshot.outputs[port].as_str(), Some("[REDACTED]"));
            }
        }
    }
}

#[test]
fn catalog_projects_once_before_dispatch_on_each_invocation_without_exposing_bindings() {
    let definition = configured(BTreeMap::from([("add".into(), addition())]));
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let graph = mf_compiler::describe_compiled(&plan).unwrap();
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
        assert!(!output.contains_key("bindings"));
        harness.traces.force_flush().unwrap();
        let spans = harness.spans.get_finished_spans().unwrap();
        assert_eq!(
            spans
                .iter()
                .filter(|span| span.attributes.iter().any(|attribute| {
                    attribute.key.as_str() == "mf.node.id"
                        && attribute.value.as_str() == "definition"
                }))
                .count(),
            1
        );
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
        for detail in &details {
            assert_eq!(detail.context.iteration_path, ["dispatch"]);
            assert!(detail.body_scope.iter().any(|entry| matches!(entry,
                mf_telemetry::event::ScopePathEntry::Flow { call_id, definition }
                if call_id == "execute" && definition.workflow_id == graph.workflow_id
                    && definition.path == vec![mf_telemetry::event::ScopePathEntry::binding_key("tool_0", "body")]
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
        edge.from_node.as_str() == "catalog" && edge.to_node.as_str() == "dispatch"
    }));
}

#[cfg(feature = "codegen")]
#[test]
fn source_free_tool_catalog_and_dispatch_runner_matches_memory() {
    use mf_compiler::{CompileRequest, SupportPackages, compile_project};
    let definition = configured(BTreeMap::from([
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
