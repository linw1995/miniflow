//! Provider-owned agent Flows. Expansion only builds ordinary graph/node definitions.
use mf_runtime::{DescribedFlowArgument, NodeBuildError, WorkflowDefinition};
use serde_json::{Value, json};
use snafu::{ResultExt, ensure};
use std::collections::BTreeMap;

type Flows = BTreeMap<String, DescribedFlowArgument>;

pub fn agent_turn_definition() -> Result<WorkflowDefinition, mf_runtime::DefinitionParseError> {
    WorkflowDefinition::from_json(include_str!("../workflows/agent-turn.json"))
}
pub fn agent_definition() -> Result<WorkflowDefinition, mf_runtime::DefinitionParseError> {
    WorkflowDefinition::from_json(include_str!("../workflows/agent.json"))
}
pub fn tool_uses_definition() -> Result<WorkflowDefinition, mf_runtime::DefinitionParseError> {
    WorkflowDefinition::from_json(include_str!("../workflows/tool-uses.json"))
}

fn parse(value: Value) -> Result<WorkflowDefinition, NodeBuildError> {
    // Generated wire format goes through the same versioned decoder as authored Flows.
    WorkflowDefinition::from_json(&value.to_string())
        .boxed()
        .map_err(NodeBuildError::from)
}

fn source(node: &str, port: &str) -> Value {
    json!({"from": {"node": node, "port": port}})
}
fn subgraph(name: &str, definition: &WorkflowDefinition) -> Value {
    json!({"flow": name, "definition": definition})
}
fn project(
    name: &str,
    binding: &DescribedFlowArgument,
) -> Result<(crate::ToolDefinition, Value), NodeBuildError> {
    crate::tools::project_tool(
        name,
        binding.description.as_deref().unwrap_or_default(),
        &binding.parameters,
        &binding.metadata,
    )
}
fn validate(flows: &Flows) -> Result<(), NodeBuildError> {
    for (name, binding) in flows {
        ensure!(
            (1..=64).contains(&name.len())
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                && name != "else",
            mf_runtime::NodeInvalidSubgraphSnafu {
                message: format!("invalid tool name `{name}`")
            }
        );
        ensure!(
            binding
                .description
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty()),
            mf_runtime::NodeInvalidSubgraphSnafu {
                message: format!("tool `{name}` requires a nonblank binding description")
            }
        );
        ensure!(
            binding.definition.execution.is_none(),
            mf_runtime::NodeInvalidSubgraphSnafu {
                message: format!("tool `{name}` requires a terminal interface")
            }
        );
    }
    Ok(())
}

fn catalog(flows: &Flows) -> Result<Value, NodeBuildError> {
    let tools = flows
        .iter()
        .map(|(name, binding)| {
            project(name, binding).map(|(definition, _)| {
                json!({
                    "name": definition.name,
                    "description": definition.description,
                    "input_schema": definition.input_schema,
                })
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"id": "tools", "kind": "builtin.constant", "config": {"value": tools}}))
}

pub fn configure_tool_uses(
    config: Value,
    flows: Flows,
) -> Result<WorkflowDefinition, NodeBuildError> {
    ensure!(
        config.is_null() || config.as_object().is_some_and(serde_json::Map::is_empty),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "tool-uses accepts injected flows but no configuration options"
        }
    );
    validate(&flows)?;
    if flows.is_empty() {
        return tool_uses_definition().boxed().map_err(NodeBuildError::from);
    }
    let mut value: Value =
        serde_json::from_str(include_str!("../workflows/tool-uses.json")).expect("builtin JSON");
    let body = &mut value["nodes"][1]["config"]["body"];
    let mut nodes = body["nodes"].as_array().cloned().expect("tool-uses nodes");
    let mut edges = body["edges"].as_array().cloned().expect("tool-uses edges");
    let route = nodes
        .iter()
        .position(|node| node["id"] == "route")
        .expect("tool-uses router");
    // The template routes every call to its placeholder `unknown` branch. Replace that branch with
    // one route per injected tool while retaining the shared `else` fallback.
    nodes[route]["config"]["routes"] = json!(flows.keys().collect::<Vec<_>>());
    nodes.retain(|node| {
        !matches!(
            node["id"].as_str(),
            Some("unknown_named" | "unknown_named_result")
        )
    });
    edges.retain(|edge| edge["from_node"] != "route");
    edges.push(json!({"from_node": "route", "from_output": "else", "to_node": "unknown_else"}));
    let mut results = vec![json!({"node": "unknown_else_result", "port": "result"})];
    for (index, (name, binding)) in flows.iter().enumerate() {
        let arguments = format!("arguments_{index}");
        let invoke = format!("invoke_{index}");
        let encode = format!("encode_{index}");
        let (_, config) = project(name, binding)?;
        let mut node = json!({"id": arguments, "kind": "llm.tool_arguments", "config": config});
        node["inputs"] = json!({"call": source("%iteration", "item")});
        nodes.push(node);
        edges.push(json!({"from_node": "route", "from_output": name, "to_node": arguments}));
        nodes.push(json!({"id": invoke, "kind": "workflow.call", "config": {"flow": name, "definition": binding.definition, "injected": true, "arguments_object": true, "capture_errors": true}, "inputs": {"arguments": source(&arguments, "arguments")}}));
        nodes.push(json!({"id": encode, "kind": "llm.tool_result", "inputs": {"call": source("%iteration", "item"), "plain_content": source(&arguments, "plain_content"), "result": {"exclusive": [{"node": arguments, "port": "error"}, {"node": invoke, "port": "result"}]}}}));
        results.push(json!({"node": encode, "port": "result"}));
    }
    let result = nodes
        .iter()
        .position(|node| node["id"] == "result")
        .expect("tool-uses result");
    nodes[result]["inputs"]["input"] = json!({"exclusive": results});
    body["nodes"] = Value::Array(nodes);
    body["edges"] = Value::Array(edges);
    parse(value)
}

pub fn configure_agent(config: Value, flows: Flows) -> Result<WorkflowDefinition, NodeBuildError> {
    ensure!(
        config.is_null() || config.as_object().is_some_and(serde_json::Map::is_empty),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "agent accepts injected flows but no configuration options"
        }
    );
    validate(&flows)?;
    let mut value: Value =
        serde_json::from_str(include_str!("../workflows/agent.json")).expect("builtin JSON");
    let nodes = value["nodes"].as_array_mut().expect("nodes");
    let position = nodes
        .iter()
        .position(|node| node["id"] == "tools")
        .expect("catalog placeholder");
    nodes[position] = catalog(&flows)?;
    let mut uses = configure_tool_uses(config.clone(), flows)?;
    // The enclosing agent supplies these providers; don't re-select registry sources in a frozen child.
    uses.dependencies.clear();
    for node in nodes {
        if node["id"] == "repeat" {
            for child in node["loop"]["body"]["nodes"]
                .as_array_mut()
                .expect("loop nodes")
            {
                if child["id"] == "dispatch" {
                    child["config"] = subgraph("tool-uses", &uses);
                }
            }
        }
    }
    // Dispatch now holds the frozen configured body; keep agent-turn's live source.
    value["flows"]
        .as_object_mut()
        .expect("builtin Flow sources")
        .remove("tool-uses");
    parse(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::FlowParameterDescription;

    fn binding(description: Option<&str>) -> DescribedFlowArgument {
        DescribedFlowArgument {
            definition: tool_uses_definition().unwrap(),
            description: description.map(str::to_owned),
            metadata: mf_runtime::SubgraphMetadata {
                inputs: mf_runtime::WorkflowInputSchema {
                    inputs: BTreeMap::from([(
                        "params".into(),
                        BTreeMap::from([(
                            "calls".into(),
                            mf_runtime::WorkflowInput {
                                value_type: mf_runtime::ValueType::Array,
                                required: true,
                                binding: Default::default(),
                            },
                        )]),
                    )]),
                    ..Default::default()
                },
                outputs: vec![],
                stream_outputs: vec![],
            },
            parameters: BTreeMap::from([(
                "params.calls".into(),
                FlowParameterDescription {
                    description: "Calls to dispatch.".into(),
                },
            )]),
        }
    }

    #[test]
    fn both_configurable_flows_require_binding_descriptions() {
        for description in [None, Some(""), Some(" \n\t")] {
            let flows = BTreeMap::from([("dispatch".into(), binding(description))]);
            assert!(configure_agent(Value::Null, flows.clone()).is_err());
            assert!(configure_tool_uses(Value::Null, flows).is_err());
        }
    }

    #[test]
    fn adapters_contain_linked_metadata_not_a_duplicate_child_definition() {
        let binding = binding(Some("Dispatch supplied calls."));
        let (_, config) = project("dispatch", &binding).unwrap();
        assert_eq!(config["description"], "Dispatch supplied calls.");
        assert_eq!(
            config["parameters"],
            json!({"params.calls": {"description": "Calls to dispatch."}})
        );
        assert_eq!(
            config["metadata"],
            serde_json::to_value(&binding.metadata).unwrap()
        );
        assert!(config.get("subgraph").is_none());
        assert!(config.get("definition").is_none());
        let uses = configure_tool_uses(
            Value::Null,
            BTreeMap::from([("dispatch".into(), binding.clone())]),
        )
        .unwrap();
        let value = serde_json::to_value(uses).unwrap();
        let body = &value["nodes"][1]["config"]["body"]["nodes"];
        let nodes = body.as_array().unwrap();
        assert!(
            nodes
                .iter()
                .any(|node| node["kind"] == "llm.tool_arguments" && node["config"] == config)
        );
        assert!(nodes.iter().any(|node| node["kind"] == "workflow.call"
            && node["config"]["definition"] == serde_json::to_value(&binding.definition).unwrap()));
    }

    #[test]
    fn catalog_is_one_constant_even_for_non_cel_identifiers() {
        let flows = BTreeMap::from([
            ("search-web".into(), binding(Some("Search the web."))),
            ("9lookup".into(), binding(Some("Look up a value."))),
        ]);
        let agent = serde_json::to_value(configure_agent(Value::Null, flows).unwrap()).unwrap();
        let nodes = agent["nodes"].as_array().unwrap();
        let tools = nodes.iter().find(|node| node["id"] == "tools").unwrap();
        assert_eq!(tools["kind"], "builtin.constant");
        assert_eq!(tools["config"]["value"][0]["name"], "9lookup");
        assert_eq!(tools["config"]["value"][1]["name"], "search-web");
        assert!(
            !nodes
                .iter()
                .any(|node| node["kind"] == "llm.tool_definition")
        );
        assert!(
            !nodes
                .iter()
                .any(|node| node["id"].as_str().unwrap_or_default().starts_with("tool_"))
        );
        assert_eq!(
            catalog(&Flows::new()).unwrap()["config"]["value"],
            json!([])
        );
    }

    #[test]
    fn projection_rejects_unknown_description_keys() {
        let mut binding = binding(Some("Dispatch calls."));
        binding.parameters.insert(
            "params.missing".into(),
            FlowParameterDescription {
                description: "Unknown".into(),
            },
        );
        let flows = BTreeMap::from([("dispatch".into(), binding)]);
        assert!(
            configure_agent(Value::Null, flows.clone())
                .unwrap_err()
                .to_string()
                .contains("describes unknown input")
        );
        assert!(
            configure_tool_uses(Value::Null, flows)
                .unwrap_err()
                .to_string()
                .contains("describes unknown input")
        );
    }
}

mf_runtime::register_builtin_flow!("agent-turn", agent_turn_definition);
mf_runtime::register_builtin_flow!("agent", agent_definition, metadata = configure_agent);
mf_runtime::register_builtin_flow!(
    "tool-uses",
    tool_uses_definition,
    metadata = configure_tool_uses
);
