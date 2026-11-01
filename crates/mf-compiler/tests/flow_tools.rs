extern crate mfn_code as _;
extern crate mfn_core as _;
extern crate mfn_llm as _;

mod common;

use mf_compiler::{
    CompileRequest, NodeRegistry, SupportPackages, WorkflowDefinition, compile_definition,
    compile_project, compile_resolved_definition, describe_compiled, instantiate_compiled,
    load_workflow,
};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn text_tool() -> Value {
    json!({
        "version": "2026-11-01",
        "dependencies": {},
        "nodes": [
            {"id": "input", "kind": "builtin.parameters", "config": {"parameters": {
                "name": {"type": "string", "secret": false},
                "punctuation": {"type": "string", "default": "!", "secret": false}
            }}},
            {"id": "format", "kind": "builtin.code", "config": {
                "language": "cel",
                "inputs": {"name": "string", "punctuation": "string"},
                "code": {"content": "'Hello, ' + name + punctuation"}
            }, "inputs": {
                "name": {"from": {"node": "input", "port": "name"}},
                "punctuation": {"from": {"node": "input", "port": "punctuation"}}
            }}
        ],
        "outputs": [{"name": "content", "node": "format", "port": "content"}]
    })
}

fn structured_tool() -> Value {
    json!({
        "version": "2026-11-01",
        "dependencies": {},
        "nodes": [
            {"id": "input", "kind": "builtin.parameters", "config": {"parameters": {
                "value": {"type": "int", "secret": false},
                "factor": {"type": "int", "default": 2, "secret": false}
            }}},
            {"id": "calculate", "kind": "builtin.code", "config": {
                "language": "cel",
                "inputs": {"value": "int", "factor": "int"},
                "code": {"doubled": "value * factor", "original": "value"}
            }, "inputs": {
                "value": {"from": {"node": "input", "port": "value"}},
                "factor": {"from": {"node": "input", "port": "factor"}}
            }}
        ],
        "outputs": [
            {"name": "doubled", "node": "calculate", "port": "doubled"},
            {"name": "original", "node": "calculate", "port": "original"}
        ]
    })
}

fn calls() -> Value {
    json!([
        {"id": "text-first", "name": "greet", "arguments": {"name": "Alice"}},
        {"id": "structured", "name": "stats", "arguments": {"value": 21, "factor": 3}},
        {"id": "unknown", "name": "missing", "arguments": {}},
        {"id": "malformed", "name": "greet", "arguments": []},
        {"id": "text-second", "name": "greet", "arguments": {"name": "Bob", "punctuation": "?"}},
        {"id": "defaulted", "name": "stats", "arguments": {"value": 7}},
        {"id": "required", "name": "greet", "arguments": {}}
    ])
}

fn parent(calls: Value) -> Value {
    json!({
        "version": "2026-11-01",
        "dependencies": {
            "core": {"package": "mfn-core", "path": common::crates_dir().join("builtin-nodes/core")},
            "code": {"package": "mfn-code", "path": common::crates_dir().join("builtin-nodes/code")},
            "llm": {"package": "mfn-llm", "path": common::crates_dir().join("builtin-nodes/llm")}
        },
        "flows": {"uses": {"builtin": "mfn-llm.tool-uses", "flows": {
            "greet": {"source": {"path": "flows/greet.json"}, "description": "Greet a person by name."},
            "stats": {"source": {"path": "flows/stats.json"}, "description": "Calculate a value and its multiple."}
        }}},
        "nodes": [
            {"id": "requests", "kind": "builtin.constant", "config": {"value": calls}},
            {"id": "dispatch", "kind": "workflow.call", "config": {"flow": "uses"}, "inputs": {"params.calls": {"from": {"node": "requests", "port": "value"}}}}
        ],
        "outputs": [{"name": "results", "node": "dispatch", "port": "results"}]
    })
}

fn write_fixture(root: &Path, parent_definition: Value) -> std::path::PathBuf {
    fs::create_dir(root.join("flows")).unwrap();
    fs::write(root.join("flows/greet.json"), text_tool().to_string()).unwrap();
    fs::write(root.join("flows/stats.json"), structured_tool().to_string()).unwrap();
    let definition = root.join("workflow.json");
    fs::write(&definition, parent_definition.to_string()).unwrap();
    definition
}

fn execute_source(
    definition: &Path,
) -> (
    mf_compiler::CompiledWorkflow,
    mf_compiler::ResolvedWorkflow,
    Value,
) {
    let resolved = load_workflow(definition).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_resolved_definition(&resolved.definition, &registry).unwrap();
    let outputs = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap();
    (plan, resolved, serde_json::to_value(outputs).unwrap())
}

fn error_content(result: &Value) -> String {
    serde_json::from_str::<Value>(result["content"].as_str().unwrap()).unwrap()["error"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn local_named_flows_freeze_definitions_expose_schemas_and_dispatch_in_order() {
    let root = tempfile::tempdir().unwrap();
    let definition = write_fixture(root.path(), parent(calls()));
    let (_plan, resolved, outputs) = execute_source(&definition);

    assert_eq!(resolved.sources.len(), 3);
    assert!(resolved.definition.flows.is_empty());
    let frozen = resolved
        .definition
        .nodes
        .iter()
        .find(|node| node.id == "dispatch".into())
        .unwrap();
    let child = &frozen.config["definition"];
    assert!(child.to_string().contains("builtin.router"));
    let dispatch = child["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "dispatch")
        .unwrap();
    let route = dispatch["config"]["body"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "route")
        .unwrap();
    assert_eq!(route["config"]["routes"], json!(["greet", "stats"]));
    assert!(child.to_string().contains("workflow.call"));
    assert!(!child.to_string().contains("workflow.tool_call"));

    let results = outputs["results"].as_array().unwrap();
    assert_eq!(results.len(), calls().as_array().unwrap().len());
    assert_eq!(
        results
            .iter()
            .map(|result| result["tool_call_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "text-first",
            "structured",
            "unknown",
            "malformed",
            "text-second",
            "defaulted",
            "required"
        ]
    );
    assert_eq!(results[0]["content"], "Hello, Alice!");
    assert_eq!(
        serde_json::from_str::<Value>(results[1]["content"].as_str().unwrap()).unwrap(),
        json!({"doubled": 63, "original": 21})
    );
    assert!(error_content(&results[2]).contains("unknown tool"));
    assert!(error_content(&results[3]).contains("arguments must be a JSON object"));
    assert_eq!(results[4]["content"], "Hello, Bob?");
    assert_eq!(
        serde_json::from_str::<Value>(results[5]["content"].as_str().unwrap()).unwrap(),
        json!({"doubled": 14, "original": 7})
    );
    assert!(error_content(&results[6]).contains("required"));

    // Resolution owns the complete child graphs; subsequent source changes cannot affect a plan.
    fs::write(root.path().join("flows/greet.json"), "not JSON").unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let frozen = compile_resolved_definition(&resolved.definition, &registry).unwrap();
    let frozen_outputs = instantiate_compiled(&frozen, &registry)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(serde_json::to_value(frozen_outputs).unwrap(), outputs);
}

#[test]
fn review_optional_content_output_is_encoded_as_structured_json() {
    let root = tempfile::tempdir().unwrap();
    let definition = write_fixture(
        root.path(),
        parent(json!([{"id": "optional", "name": "greet", "arguments": {"name": "Alice"}}])),
    );
    let mut greet = text_tool();
    greet["outputs"][0]["optional"] = json!(true);
    fs::write(root.path().join("flows/greet.json"), greet.to_string()).unwrap();
    let (_, _, outputs) = execute_source(&definition);
    assert_eq!(
        serde_json::from_str::<Value>(outputs["results"][0]["content"].as_str().unwrap()).unwrap(),
        json!({"content": "Hello, Alice!"})
    );
}

#[test]
fn tool_bindings_require_descriptions_but_children_need_no_new_version_or_annotation() {
    for version in ["2026-10-28", "2026-11-01"] {
        let annotated = json!({
            "version": version,
            "annotation": "Documentation belongs to the caller binding.",
            "dependencies": {},
            "nodes": []
        });
        assert!(
            WorkflowDefinition::from_json(&annotated.to_string()).is_err(),
            "top-level annotation must be rejected for {version}"
        );
    }

    for description in [None, Some(" \n\t")] {
        let root = tempfile::tempdir().unwrap();
        let mut caller = parent(json!([]));
        let binding = &mut caller["flows"]["uses"]["flows"]["greet"];
        if let Some(description) = description {
            binding["description"] = json!(description);
        } else {
            binding.as_object_mut().unwrap().remove("description");
        }
        let definition = write_fixture(root.path(), caller);
        let error = load_workflow(&definition).err().unwrap().to_string();
        assert!(error.contains("nonblank binding description"), "{error}");
        assert!(error.contains("greet"), "{error}");
    }

    let root = tempfile::tempdir().unwrap();
    let definition = write_fixture(
        root.path(),
        parent(json!([{"id": "old", "name": "greet", "arguments": {"name": "Alice"}}])),
    );
    let mut old_child = text_tool();
    old_child["version"] = json!("2026-10-28");
    fs::write(root.path().join("flows/greet.json"), old_child.to_string()).unwrap();
    let (_, _, outputs) = execute_source(&definition);
    assert_eq!(outputs["results"][0]["content"], "Hello, Alice!");
}

#[test]
fn flow_tool_bodies_reject_multiple_startups_stdin_and_streaming_interfaces() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (child, expected) in [
        (
            json!({
                "version": "2026-11-01",
                "dependencies": {},
                "nodes": [
                    {"id": "first", "kind": "builtin.constant", "config": {"value": "first"}},
                    {"id": "second", "kind": "builtin.constant", "config": {"value": "second"}}
                ],
                "outputs": [{"name": "content", "node": "first", "port": "value"}]
            }),
            "exactly one startup node",
        ),
        (
            json!({
                "version": "2026-11-01",
                "dependencies": {},
                "nodes": [{"id": "read", "kind": "builtin.readline"}],
                "outputs": [{"name": "content", "node": "read", "port": "line"}]
            }),
            "requires streaming execution",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let definition = write_fixture(root.path(), parent(json!([])));
        fs::write(root.path().join("flows/greet.json"), child.to_string()).unwrap();
        let resolved = load_workflow(&definition).unwrap();
        let error = compile_resolved_definition(&resolved.definition, &registry)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
    }

    let root = tempfile::tempdir().unwrap();
    let definition = write_fixture(root.path(), parent(json!([])));
    fs::write(
        root.path().join("flows/greet.json"),
        json!({
            "version": "2026-11-01",
            "execution": {},
            "dependencies": {},
            "nodes": [{"id": "source", "kind": "builtin.constant", "config": {"value": "text"}}],
            "outputs": [{"name": "content", "node": "source", "port": "value"}]
        })
        .to_string(),
    )
    .unwrap();
    let error = load_workflow(&definition).err().unwrap().to_string();
    assert!(error.contains("terminal interface"), "{error}");
}

#[test]
fn tool_argument_errors_and_child_failures_recover_without_executing_other_routes() {
    let root = tempfile::tempdir().unwrap();
    let definition = write_fixture(
        root.path(),
        parent(json!([
            {"id":"unknown", "name":"greet", "arguments":{"name":"Alice", "extra":1}},
            {"id":"wrong-type", "name":"greet", "arguments":{"name":12}},
            {"id":"failure", "name":"stats", "arguments":{"value":1}}
        ])),
    );
    let mut failing = structured_tool();
    failing["nodes"][1]["config"]["code"]["doubled"] = json!("value / 0");
    fs::write(root.path().join("flows/stats.json"), failing.to_string()).unwrap();
    let (_, _, outputs) = execute_source(&definition);
    let results = outputs["results"].as_array().unwrap();
    assert!(error_content(&results[0]).contains("unknown or secret"));
    assert!(error_content(&results[1]).contains("name"));
    assert!(!error_content(&results[2]).is_empty());
}

#[test]
fn agent_builtin_without_tools_and_empty_tool_uses_are_valid() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let empty = mfn_llm::flows::tool_uses_definition().unwrap();
    assert_eq!(
        mfn_llm::flows::configure_tool_uses(Value::Null, Default::default()).unwrap(),
        empty
    );
    let body = &empty.nodes[1].config["body"];
    assert!(
        body["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["kind"] == "builtin.router")
    );
    assert_eq!(body["nodes"][1]["config"]["routes"], json!(["unknown"]));
    for (name, input) in [("mfn-llm.agent", "prompt"), ("mfn-llm.tool-uses", "calls")] {
        let definition = WorkflowDefinition::from_json(
            &json!({
                "version":"2026-11-01", "dependencies":{},
                "flows":{"child":{"builtin":name}},
                "nodes":[{"id":"child", "kind":"workflow.call", "config":{"flow":"child"}}]
            })
            .to_string(),
        )
        .unwrap();
        let plan = mf_compiler::compile_definition(&definition, &registry).unwrap();
        assert!(
            plan.definition.nodes[0].config["definition"]
                .to_string()
                .contains(input)
        );
    }
}

#[test]
fn empty_tool_uses_routes_every_call_to_the_unknown_fallback() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition = mfn_llm::flows::tool_uses_definition().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let outputs = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_with_inputs(
            mf_runtime::WorkflowArguments::try_from(json!({"params.calls": [
                {"id": "named", "name": "unknown", "arguments": {}},
                {"id": "other", "name": "missing", "arguments": {}}
            ]}))
            .unwrap(),
        )
        .unwrap();
    let results = serde_json::to_value(&outputs).unwrap();
    let results = results["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    for (result, id) in results.iter().zip(["named", "other"]) {
        assert_eq!(result["tool_call_id"], id);
        assert_eq!(error_content(result), "unknown tool");
    }
}

#[test]
fn generated_runner_matches_memory_after_all_flow_sources_and_build_are_removed() {
    let root = tempfile::tempdir().unwrap();
    let mut requests = calls();
    requests.as_array_mut().unwrap().push(json!({
        "id": "inline", "name": "add", "arguments": {"left": 2, "right": 3}
    }));
    let mut caller = parent(requests);
    caller["flows"]["uses"]["flows"]["add"] = json!({
        "source": {"body": addition_body()}, "description": "Add two integers.",
        "parameters": {"add.left": {"description": "First operand."}}
    });
    let definition = write_fixture(root.path(), caller);
    let (_plan, _resolved, expected) = execute_source(&definition);
    let output = root.path().join("runner");
    let build = root.path().join("build");
    compile_project(&CompileRequest {
        definition: &definition,
        output: &output,
        locked: false,
        build_dir: Some(&build),
        support: &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    })
    .unwrap();

    fs::remove_file(&definition).unwrap();
    fs::remove_file(root.path().join("flows/greet.json")).unwrap();
    fs::remove_file(root.path().join("flows/stats.json")).unwrap();
    fs::remove_dir_all(&build).unwrap();
    assert!(!definition.exists());
    assert!(!root.path().join("flows/greet.json").exists());
    assert!(!root.path().join("flows/stats.json").exists());
    assert!(!build.exists());

    let result = Command::new(&output)
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
fn calculator_example_uses_provider_agent_and_native_loop() {
    use mfn_llm::test_support::{Reply, Server};
    let mut first = Reply::text("");
    let mut response: Value = serde_json::from_str(&first.body).unwrap();
    response["choices"][0]["message"]["tool_calls"] = json!([
        {"id":"add", "type":"function", "function":{"name":"calculator", "arguments":"{\"operation\":\"add\",\"left\":2,\"right\":3}"}}
    ]);
    response["choices"][0]["finish_reason"] = json!("tool_calls");
    first.body = response.to_string();
    let server = Server::start(vec![first, Reply::text("Five")]);
    let example = common::crates_dir()
        .parent()
        .unwrap()
        .join("examples/chat-calculator-flow.json");
    let resolved = load_workflow(&example).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut definition = resolved.definition;
    definition.outputs.retain(|output| output.name != "answer");
    let plan = compile_resolved_definition(&definition, &registry).unwrap();
    let outputs = instantiate_compiled(&plan, &registry).unwrap().execute_with_inputs(mf_runtime::WorkflowArguments::try_from(json!({
        "params.prompt":"Add two and three", "params.model":"example-model", "params.base_url":server.base_url, "params.api_key":"synthetic"
    })).unwrap()).unwrap();
    assert_eq!(outputs["text"].as_str(), Some("Five"));
    assert_eq!(outputs["status"].as_str(), Some("succeeded"));
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].body["tools"][0]["function"]["name"],
        "calculator"
    );
    assert_eq!(requests[1].body["messages"][2]["role"], "tool");
}

#[test]
fn injected_flow_bodies_are_marked_for_inspection() {
    let example = common::crates_dir()
        .parent()
        .unwrap()
        .join("examples/chat-calculator-flow.json");
    let resolved = load_workflow(&example).unwrap();
    let plan = mf_compiler::plan_definition(&resolved.definition).unwrap();
    let description = describe_compiled(&plan).unwrap();
    assert_eq!(description.injected_bodies.len(), 1);
    let path = &description.injected_bodies[0];
    assert_eq!(path.last().map(String::as_str), Some("invoke_0"));
    let body = description.body(path).unwrap();
    assert_eq!(
        body.nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        ["calculate"]
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let manifest = plan.manifest(&registry).unwrap();
    assert_eq!(
        manifest.description().unwrap().injected_bodies,
        description.injected_bodies
    );
}

#[test]
fn generic_injected_call_provenance_does_not_change_execution() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for injected in [false, true] {
        let definition = WorkflowDefinition::from_json(
            &json!({
                "version": "2026-11-01", "dependencies": {},
                "nodes": [{"id": "call", "kind": "workflow.call", "config": {
                    "flow": "child", "injected": injected,
                    "definition": {
                        "version": "2026-11-01", "dependencies": {},
                        "nodes": [{"id": "value", "kind": "builtin.constant", "config": {"value": "same"}}],
                        "outputs": [{"name": "value", "node": "value", "port": "value"}]
                    }
                }}],
                "outputs": [{"name": "value", "node": "call", "port": "value"}]
            })
            .to_string(),
        )
        .unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        assert_eq!(
            describe_compiled(&plan).unwrap().injected_bodies.len(),
            usize::from(injected)
        );
        let outputs = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap();
        assert_eq!(outputs["value"].as_str(), Some("same"));
    }
}

#[test]
fn injection_markers_survive_noncanonical_source_order() {
    let root = tempfile::tempdir().unwrap();
    let path = write_fixture(root.path(), parent(calls()));
    for file in ["greet", "stats"] {
        let path = root.path().join(format!("flows/{file}.json"));
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["nodes"].as_array_mut().unwrap().reverse();
        if let Some(outputs) = value.get_mut("outputs").and_then(Value::as_array_mut) {
            outputs.reverse();
        }
        fs::write(path, value.to_string()).unwrap();
    }
    let resolved = load_workflow(&path).unwrap();
    let plan = mf_compiler::plan_definition(&resolved.definition).unwrap();
    assert_eq!(describe_compiled(&plan).unwrap().injected_bodies.len(), 2);
    let registry = NodeRegistry::from_inventory().unwrap();
    assert_eq!(
        plan.manifest(&registry)
            .unwrap()
            .description()
            .unwrap()
            .injected_bodies
            .len(),
        2
    );
}

#[test]
fn builtin_configuration_and_metadata_errors_keep_typed_boundaries() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for source in [
        json!({"builtin":"mfn-llm.agent-turn", "config":{"unexpected":true}}),
        json!({"builtin":"mfn-llm.tool-uses", "config":{"unexpected":true}}),
        json!({"builtin":"mfn-llm.tool-uses", "flows":{"else":{"source":{"builtin":"mfn-llm.agent-turn"},"description":"Invalid reserved tool name."}}}),
    ] {
        let definition = WorkflowDefinition::from_json(&json!({"version":"2026-11-01", "dependencies":{}, "flows":{"child":source}, "nodes":[{"id":"child", "kind":"workflow.call", "config":{"flow":"child"}}]}).to_string()).unwrap();
        assert!(mf_compiler::compile_definition(&definition, &registry).is_err());
    }
    let metadata = json!({"subgraph":{"flow":"tool", "definition":text_tool()}, "name":"tool", "description":"Description"}).to_string();
    assert!(
        mf_compiler::instantiate_metadata_with_metadata(&registry, "x", "missing", &metadata)
            .is_err()
    );
    assert!(
        mf_compiler::instantiate_metadata_with_metadata(
            &registry,
            "x",
            "llm.tool_definition",
            "not json"
        )
        .is_err()
    );
    assert!(
        mf_compiler::instantiate_metadata_with_metadata(
            &registry,
            "x",
            "builtin.identity",
            &metadata
        )
        .is_err()
    );
    let prepared = mf_compiler::instantiate_metadata_with_metadata(
        &registry,
        "x",
        "llm.tool_definition",
        &metadata,
    )
    .unwrap();
    assert!(prepared.metadata.ports.inputs.is_empty());
}

fn addition_body() -> Value {
    json!({
        "nodes": [{"id": "add", "kind": "builtin.code", "config": {
            "language": "cel", "inputs": {"left": "int", "right": "int"},
            "code": {"sum": "left + right"}
        }}],
        "outputs": [{"name": "sum", "node": "add", "port": "sum"}]
    })
}

fn builtin_addition() -> Result<WorkflowDefinition, mf_runtime::DefinitionParseError> {
    let mut definition = addition_body();
    definition["version"] = json!("2026-10-28");
    definition["dependencies"] = json!({});
    WorkflowDefinition::from_json(&definition.to_string())
}

mf_runtime::register_builtin_flow!("test-addition", builtin_addition);

fn agent_parent(flows: Value) -> Value {
    json!({
        "version": "2026-11-01", "dependencies": {},
        "flows": {"agent": {"builtin": "mfn-llm.agent", "flows": flows}},
        "nodes": [{"id": "agent", "kind": "workflow.call", "config": {"flow": "agent"}}],
        "outputs": [{"name": "text", "node": "agent", "port": "text"},
                    {"name": "status", "node": "agent", "port": "status"}]
    })
}

#[test]
fn inline_single_code_tool_exposes_exact_schema_and_maps_short_arguments() {
    use mfn_llm::test_support::{Reply, Server};
    let mut first = Reply::text("");
    let mut response: Value = serde_json::from_str(&first.body).unwrap();
    response["choices"][0]["message"]["tool_calls"] = json!([
        {"id": "addition", "type": "function", "function": {
            "name": "add", "arguments": "{\"left\":2,\"right\":3}"
        }}
    ]);
    response["choices"][0]["finish_reason"] = json!("tool_calls");
    first.body = response.to_string();
    let server = Server::start(vec![first, Reply::text("Five")]);
    let definition = WorkflowDefinition::from_json(
        &agent_parent(json!({
            "add": {
                "source": {"body": addition_body()}, "description": "Add two integers.",
                "parameters": {
                    "add.left": {"description": "First operand."},
                    "add.right": {"description": "Second operand."}
                }
            }
        }))
        .to_string(),
    )
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let outputs = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_with_inputs(
            mf_runtime::WorkflowArguments::try_from(json!({
                "agent.params.prompt": "Add two and three", "agent.params.model": "example-model",
                "agent.params.base_url": server.base_url, "agent.params.api_key": "synthetic"
            }))
            .unwrap(),
        )
        .unwrap();
    assert_eq!(outputs["text"].as_str(), Some("Five"));
    assert_eq!(outputs["status"].as_str(), Some("succeeded"));
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].body["tools"],
        json!([{
            "type": "function", "function": {
                "name": "add", "description": "Add two integers.",
                "parameters": {
                    "type": "object", "properties": {
                        "left": {"type": "integer", "description": "First operand."},
                        "right": {"type": "integer", "description": "Second operand."}
                    },
                    "required": ["left", "right"], "additionalProperties": false
                }
            }
        }])
    );
    let result = &requests[1].body["messages"][2];
    assert_eq!(result["role"], "tool");
    assert_eq!(result["tool_call_id"], "addition");
    assert_eq!(
        serde_json::from_str::<Value>(result["content"].as_str().unwrap()).unwrap(),
        json!({"sum": 5})
    );
}

#[test]
fn inline_multi_node_body_preserves_input_bindings_through_roundtrip_and_execution() {
    let mut caller = parent(json!([
        {"id": "addition", "name": "greet", "arguments": {"left": 2, "right": 3}}
    ]));
    let body = json!({
        "nodes": [
            {"id": "params", "kind": "builtin.parameters", "config": {"parameters": {
                "left": {"type": "int", "secret": false},
                "right": {"type": "int", "secret": false}
            }}},
            {"id": "add", "kind": "builtin.code", "config": {
                "language": "cel", "inputs": {"left": "int", "right": "int"},
                "code": {"sum": "left + right"}
            }, "inputs": {
                "left": {"from": {"node": "params", "port": "left"}},
                "right": {"from": {"node": "params", "port": "right"}}
            }}
        ],
        "edges": [],
        "outputs": [{"name": "sum", "node": "add", "port": "sum"}],
        "clusters": []
    });
    caller["flows"]["uses"]["flows"]["greet"] = json!({
        "source": {"body": body}, "description": "Add two integers."
    });
    let parsed = WorkflowDefinition::from_json(&caller.to_string()).unwrap();
    let encoded = serde_json::to_value(&parsed).unwrap();
    let roundtripped_body = &encoded["flows"]["uses"]["flows"]["greet"]["source"]["body"];
    assert_eq!(
        roundtripped_body["nodes"][1]["inputs"],
        body["nodes"][1]["inputs"]
    );
    assert_eq!(
        WorkflowDefinition::from_json(&encoded.to_string()).unwrap(),
        parsed
    );
    let root = tempfile::tempdir().unwrap();
    let path = write_fixture(root.path(), encoded);
    let (_, _, outputs) = execute_source(&path);
    assert_eq!(
        serde_json::from_str::<Value>(outputs["results"][0]["content"].as_str().unwrap()).unwrap(),
        json!({"sum": 5})
    );
}

#[test]
fn builtin_tool_source_uses_caller_description_and_short_arguments() {
    let mut caller = parent(json!([
        {"id": "addition", "name": "greet", "arguments": {"left": 2, "right": 3}}
    ]));
    caller["flows"]["uses"]["flows"]["greet"] = json!({
        "source": {"builtin": "mf-compiler.test-addition"},
        "description": "Add using a provider-owned Flow.",
        "parameters": {"add.left": {"description": "First operand."}}
    });
    let root = tempfile::tempdir().unwrap();
    let path = write_fixture(root.path(), caller);
    let (_, _, outputs) = execute_source(&path);
    assert_eq!(
        serde_json::from_str::<Value>(outputs["results"][0]["content"].as_str().unwrap()).unwrap(),
        json!({"sum": 5})
    );
    let builtin = builtin_addition().unwrap();
    assert!(
        serde_json::to_value(builtin)
            .unwrap()
            .get("description")
            .is_none()
    );
}

#[test]
fn tool_parameter_descriptions_reject_unknown_or_unqualified_input_names() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for builtin in ["mfn-llm.agent", "mfn-llm.tool-uses"] {
        for parameter in ["left", "add.missing"] {
            let mut caller = agent_parent(json!({
                "add": {"source": {"body": addition_body()}, "description": "Add two integers.",
                        "parameters": {parameter: {"description": "Unknown input."}}}
            }));
            caller["flows"]["agent"]["builtin"] = json!(builtin);
            caller["outputs"] = json!([]);
            let definition = WorkflowDefinition::from_json(&caller.to_string()).unwrap();
            let error = compile_definition(&definition, &registry)
                .unwrap_err()
                .to_string();
            assert!(error.contains("unknown input"), "{error}");
            assert!(error.contains(parameter), "{error}");
        }
    }
}

#[test]
fn callers_can_describe_the_same_child_differently_without_changing_it() {
    use mfn_llm::test_support::{Reply, Server};
    let root = tempfile::tempdir().unwrap();
    let path = write_fixture(
        root.path(),
        agent_parent(json!({
            "friendly": {"source": {"path": "flows/greet.json"}, "description": "Offer a friendly greeting.",
                         "parameters": {"input.name": {"description": "Friend's name."}}},
            "formal": {"source": {"path": "flows/greet.json"}, "description": "Offer a formal greeting.",
                       "parameters": {"input.name": {"description": "Recipient's name."}}}
        })),
    );
    let original = fs::read(root.path().join("flows/greet.json")).unwrap();
    let server = Server::start(vec![Reply::text("No tool needed")]);
    let resolved = load_workflow(&path).unwrap();
    fn injected_definitions(value: &Value, definitions: &mut Vec<Value>) {
        match value {
            Value::Object(fields) => {
                if fields.get("kind").and_then(Value::as_str) == Some("workflow.call")
                    && value["config"]["injected"] == true
                {
                    definitions.push(value["config"]["definition"].clone());
                }
                for value in fields.values() {
                    injected_definitions(value, definitions);
                }
            }
            Value::Array(values) => {
                for value in values {
                    injected_definitions(value, definitions);
                }
            }
            _ => {}
        }
    }
    let mut children = Vec::new();
    injected_definitions(
        &serde_json::to_value(&resolved.definition).unwrap(),
        &mut children,
    );
    assert_eq!(children.len(), 2);
    assert_eq!(children[0], children[1]);
    for child in &children {
        assert!(child.get("description").is_none());
        assert!(child.get("annotation").is_none());
    }
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_resolved_definition(&resolved.definition, &registry).unwrap();
    instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_with_inputs(
            mf_runtime::WorkflowArguments::try_from(json!({
                "agent.params.prompt": "Hello", "agent.params.model": "example-model",
                "agent.params.base_url": server.base_url, "agent.params.api_key": "synthetic"
            }))
            .unwrap(),
        )
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let tools = requests[0].body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);
    for (name, description, parameter_description) in [
        ("friendly", "Offer a friendly greeting.", "Friend's name."),
        ("formal", "Offer a formal greeting.", "Recipient's name."),
    ] {
        let function = &tools
            .iter()
            .find(|tool| tool["function"]["name"] == name)
            .unwrap()["function"];
        assert_eq!(function["description"], description);
        assert_eq!(
            function["parameters"]["properties"]["name"]["description"],
            parameter_description
        );
    }
    assert_eq!(
        fs::read(root.path().join("flows/greet.json")).unwrap(),
        original
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&original).unwrap(),
        text_tool()
    );
}

#[test]
fn agent_without_injected_tools_completes_a_direct_answer() {
    use mfn_llm::test_support::{Reply, Server};
    let server = Server::start(vec![Reply::text("Direct answer")]);
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition = WorkflowDefinition::from_json(&json!({
        "version":"2026-11-01", "dependencies":{},
        "flows":{"agent":{"builtin":"mfn-llm.agent"}},
        "nodes":[{"id":"agent", "kind":"workflow.call", "config":{"flow":"agent"}}],
        "outputs":[{"name":"text","node":"agent","port":"text"},{"name":"status","node":"agent","port":"status"}]
    }).to_string()).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let outputs = instantiate_compiled(&plan, &registry).unwrap().execute_with_inputs(mf_runtime::WorkflowArguments::try_from(json!({
        "agent.params.prompt":"Hello", "agent.params.model":"example-model", "agent.params.base_url":server.base_url, "agent.params.api_key":"synthetic"
    })).unwrap()).unwrap();
    assert_eq!(outputs["text"].as_str(), Some("Direct answer"));
    assert_eq!(outputs["status"].as_str(), Some("succeeded"));
    assert_eq!(server.requests().len(), 1);
    assert!(
        server.requests()[0]
            .body
            .get("tools")
            .is_none_or(|tools| tools.as_array().is_some_and(Vec::is_empty))
    );
}
