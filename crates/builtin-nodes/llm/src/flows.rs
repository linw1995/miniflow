//! Provider-owned agent Flows. Expansion only builds ordinary graph/node definitions.
use mf_runtime::{NodeBuildError, ResolvedFlowArgument, WorkflowDefinition};
use serde_json::{Value, json};
use snafu::{ResultExt, ensure};
use std::collections::BTreeMap;

type Flows = BTreeMap<String, ResolvedFlowArgument>;

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
fn adapter(kind: &str, id: &str, name: &str, binding: &ResolvedFlowArgument) -> Value {
    json!({"id": id, "kind": kind, "config": {"name": name, "description": binding.description, "parameters": binding.parameters, "subgraph": subgraph(name, &binding.definition)}})
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

fn catalog(flows: &Flows) -> Vec<Value> {
    let mut nodes = Vec::new();
    let mut inputs = serde_json::Map::new();
    let mut types = serde_json::Map::new();
    for (name, binding) in flows {
        // One metadata adapter per tool derives its definition; the CEL node below
        // collects those definitions into the list consumed by the LLM node.
        let id = format!("tool_{name}");
        nodes.push(adapter("llm.tool_definition", &id, name, binding));
        inputs.insert(id.clone(), source(&id, "tool"));
        types.insert(id, json!("object"));
    }
    let expression = format!(
        "[{}]",
        types
            .keys()
            .map(|name| format!("dyn({name})"))
            .collect::<Vec<_>>()
            .join(",")
    );
    if flows.is_empty() {
        nodes.push(json!({"id": "tools", "kind": "builtin.constant", "config": {"value": []}}));
    } else {
        nodes.push(json!({"id": "tools", "kind": "builtin.code", "config": {"language": "cel", "inputs": types, "code": {"value": expression}}, "inputs": inputs}));
    }
    nodes
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
        let mut node = adapter("llm.tool_arguments", &arguments, name, binding);
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
    nodes.splice(position..=position, catalog(&flows));
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

    fn binding(description: Option<&str>) -> ResolvedFlowArgument {
        ResolvedFlowArgument {
            definition: tool_uses_definition().unwrap(),
            description: description.map(str::to_owned),
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
    fn adapters_forward_binding_metadata_without_changing_the_definition() {
        let binding = binding(Some("Dispatch supplied calls."));
        for kind in ["llm.tool_definition", "llm.tool_arguments"] {
            let node = adapter(kind, "adapter", "dispatch", &binding);
            assert_eq!(node["config"]["description"], "Dispatch supplied calls.");
            assert_eq!(
                node["config"]["parameters"],
                json!({"params.calls": {"description": "Calls to dispatch."}})
            );
            assert_eq!(
                node["config"]["subgraph"]["definition"],
                serde_json::to_value(&binding.definition).unwrap()
            );
        }
    }
}

mf_runtime::register_builtin_flow!("agent-turn", agent_turn_definition);
mf_runtime::register_builtin_flow!("agent", agent_definition, configure_agent);
mf_runtime::register_builtin_flow!("tool-uses", tool_uses_definition, configure_tool_uses);
