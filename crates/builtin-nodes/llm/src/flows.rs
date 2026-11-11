//! Provider-owned Agent composition using ordinary native Flow values.
//!
//! Tool bodies are owned by existing Flow constants. Only JSON metadata enters
//! CEL; Iteration shared inputs carry each callable directly to the Router.

use crate::tools::ParameterDescription;
use mf_runtime::{DefinitionParseError, WorkflowDefinition};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A caller-described tool, independent of its executable definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowTool {
    pub description: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterDescription>,
    /// Resolve sources in their original directory before composing if it differs
    /// from the composed workflow's directory.
    pub body: WorkflowDefinition,
}

fn parse(value: Value) -> Result<WorkflowDefinition, DefinitionParseError> {
    WorkflowDefinition::from_json(&value.to_string())
}

fn template(source: &str) -> Value {
    serde_json::from_str(source).expect("provider-owned Flow template is valid JSON")
}

fn node<'a>(definition: &'a mut Value, id: &str) -> &'a mut Value {
    definition["nodes"]
        .as_array_mut()
        .expect("Flow nodes")
        .iter_mut()
        .find(|node| node["id"] == id)
        .expect("provider-owned node")
}

fn edge(node: &str, port: &str) -> Value {
    json!({"from":{"node":node,"port":port}})
}

fn flow_port(index: usize) -> String {
    format!("flow_{index}")
}

/// The dispatch-only Flow accepts JSON calls/bindings and independent Flow inputs.
/// Its single native Iteration shares those inputs with every item; no capability
/// is packed into an item or sent to CEL.
fn dispatch(tools: &BTreeMap<String, FlowTool>) -> Value {
    let mut definition = template(include_str!("../workflows/tool-uses.json"));
    for (index, name) in tools.keys().enumerate() {
        let port = flow_port(index);
        node(&mut definition, "params")["config"]["parameters"][&port] =
            json!({"type":"flow","secret":false});
        let iteration = node(&mut definition, "dispatch");
        iteration["config"]["inputs"][&port] = json!("flow");
        iteration["inputs"][&port] = edge("params", &port);
        let execute = node(&mut iteration["config"]["body"], "execute");
        execute["config"]["flow_inputs"][name] = json!(port);
        execute["inputs"][&port] = edge("%iteration", &port);
    }
    definition
}

/// Add ordinary producers and business projections, then collect only their JSON
/// outputs. Projection validates every real interface before dispatch/model work.
fn catalog(definition: &mut Value, tools: &BTreeMap<String, FlowTool>) {
    let mut inputs = serde_json::Map::new();
    let mut bindings = serde_json::Map::new();
    let mut types = serde_json::Map::new();
    let mut definitions = Vec::new();
    let mut entries = Vec::new();
    for (index, (name, tool)) in tools.iter().enumerate() {
        let producer = format!("tool_{index}");
        let projection = format!("definition_{index}");
        definition["nodes"].as_array_mut().unwrap().extend([
            json!({"id":producer,"kind":"builtin.constant","config":{"type":"flow","value":tool.body}}),
            json!({"id":projection,"kind":"llm.tool_definition","config":{
                "name":name,"description":tool.description,"parameters":tool.parameters
            },"inputs":{"flow":edge(&producer,"value")}}),
        ]);
        let binding = format!("binding_{index}");
        inputs.insert(projection.clone(), edge(&projection, "definition"));
        inputs.insert(binding.clone(), edge(&projection, "binding"));
        types.insert(projection.clone(), json!("object"));
        types.insert(binding.clone(), json!("object"));
        definitions.push(projection);
        entries.push(format!(
            "{}:{binding}",
            serde_json::to_string(name).unwrap()
        ));
    }
    bindings.insert(
        "tools".into(),
        json!(format!("[{}]", definitions.join(","))),
    );
    bindings.insert(
        "bindings".into(),
        json!(format!("dyn({{{}}})", entries.join(","))),
    );
    let projection = json!({"id":"catalog","kind":"builtin.code","config":{
        "language":"cel","inputs":types,"code":bindings
    },"inputs":inputs});
    if definition["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["id"] == "catalog")
    {
        *node(definition, "catalog") = projection;
    } else {
        definition["nodes"].as_array_mut().unwrap().push(projection);
    }
}

/// Compose a catalog and ordered parallel tool dispatch without LLM plumbing nodes.
/// Body preparation never runs tools. Interface validation/projection occurs once
/// per invocation before any dispatch, using the actual native callable metadata.
pub fn tool_uses_definition(
    tools: BTreeMap<String, FlowTool>,
) -> Result<WorkflowDefinition, DefinitionParseError> {
    let mut definition = dispatch(&tools);
    // These inputs are supplied by ordinary producers within this composition.
    node(&mut definition, "params")["config"]["parameters"] =
        json!({"calls":{"type":{"list":"object"},"secret":false}});
    catalog(&mut definition, &tools);
    node(&mut definition, "dispatch")["inputs"]["bindings"] = edge("catalog", "bindings");
    for index in 0..tools.len() {
        node(&mut definition, "dispatch")["inputs"][flow_port(index)] =
            edge(&format!("tool_{index}"), "value");
    }
    definition["edges"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from_node":"catalog","from_output":"tools","to_node":"dispatch"}));
    definition["outputs"]
        .as_array_mut()
        .unwrap()
        .insert(0, json!({"name":"catalog","node":"catalog","port":"tools"}));
    parse(definition)
}

/// One provider request with the shared chat/streaming contract.
pub fn agent_turn_definition() -> Result<WorkflowDefinition, DefinitionParseError> {
    WorkflowDefinition::from_json(include_str!("../workflows/agent-turn.json"))
}

/// Compose the original bounded native Agent Loop around caller-supplied tools.
/// Catalog projection is outside the Loop. Each turn shares immutable Flow values
/// through normal Loop state, Call inputs and Iteration inputs; only JSON reaches CEL.
pub fn agent_definition(
    tools: BTreeMap<String, FlowTool>,
) -> Result<WorkflowDefinition, DefinitionParseError> {
    let mut definition = template(include_str!("../workflows/agent.json"));
    catalog(&mut definition, &tools);
    let repeat = node(&mut definition, "repeat");
    repeat["inputs"]["tools"] = edge("catalog", "tools");
    repeat["inputs"]["bindings"] = edge("catalog", "bindings");
    let mut dispatch_definition = dispatch(&tools);
    // Inline provider bodies inherit the caller's dependency sources. Repeating
    // version declarations here would conflict with local path overrides.
    dispatch_definition["dependencies"] = json!({});
    node(&mut repeat["config"]["body"], "dispatch")["config"]["body"] = dispatch_definition;
    for index in 0..tools.len() {
        let port = flow_port(index);
        repeat["config"]["variables"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":port,"type":"flow"}));
        repeat["inputs"][&port] = edge(&format!("tool_{index}"), "value");
        node(&mut repeat["config"]["body"], "dispatch")["inputs"][format!("params.{port}")] =
            edge("%loop", &port);
    }
    parse(definition)
}

fn empty_agent() -> Result<WorkflowDefinition, DefinitionParseError> {
    agent_definition(BTreeMap::new())
}
mf_runtime::register_builtin_flow!("agent-turn", agent_turn_definition);
mf_runtime::register_builtin_flow!("agent", empty_agent);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composition_uses_existing_constants_and_only_business_adapters() {
        let tool = FlowTool {
            description: "A tool".into(),
            parameters: BTreeMap::new(),
            body: parse(json!({"version":"2026-10-28","nodes":[],"outputs":[]})).unwrap(),
        };
        let definition = tool_uses_definition(BTreeMap::from([("lookup".into(), tool)])).unwrap();
        let value = serde_json::to_value(&definition).unwrap();
        let producer = definition
            .nodes
            .iter()
            .find(|node| node.id.as_str() == "tool_0")
            .unwrap();
        assert_eq!(producer.kind, "builtin.constant");
        assert_eq!(producer.config["type"], "flow");
        assert!(
            definition
                .nodes
                .iter()
                .any(|node| node.kind == "llm.tool_definition")
        );
        for removed in [
            "llm.tool\"",
            "llm.tool_set",
            "llm.tool_calls",
            "llm.tool_call\"",
        ] {
            assert!(!value.to_string().contains(removed));
        }
        let dispatch = value["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "dispatch")
            .unwrap();
        assert_eq!(
            dispatch["config"]["inputs"],
            json!({"bindings":"object","flow_0":"flow"})
        );
        let execute = dispatch["config"]["body"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "execute")
            .unwrap();
        assert_eq!(execute["config"]["flow_inputs"], json!({"lookup":"flow_0"}));
    }
    #[test]
    fn provider_agent_turn_keeps_streaming_answer_and_empty_agent_is_composable() {
        let turn = agent_turn_definition().unwrap();
        assert!(
            turn.outputs
                .iter()
                .any(|output| output.name == "answer" && output.port == "delta" && output.optional)
        );
        let agent = empty_agent().unwrap();
        assert!(
            agent
                .nodes
                .iter()
                .any(|node| node.kind == mf_runtime::LOOP_KIND)
        );
        assert_eq!(agent.outputs.len(), 5);
    }
}
