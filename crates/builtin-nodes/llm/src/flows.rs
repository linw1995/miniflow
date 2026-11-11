//! Provider-owned composition using native callable values on ordinary data edges.
//!
//! Source loading remains the caller's responsibility. The generated definition
//! uses one producer-owned body per tool and the normal resolver and compiler.

use crate::tools::ParameterDescription;
use mf_runtime::{DefinitionParseError, EdgeDefinition, NodeDefinition, WorkflowDefinition};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

/// One caller-described tool. Descriptions belong to this binding, not its body.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowTool {
    pub description: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterDescription>,
    /// A self-contained definition, with its own dependencies and source aliases.
    /// Resolve external sources before composing when their base directory differs
    /// from the composed workflow's source directory.
    pub body: WorkflowDefinition,
}

/// Compose a catalog and native parallel dispatch from the same prepared tools.
///
/// Each tool has one producer with a direct `body` binding. Preparation checks its
/// actual interface without executing the tool. At run time, its native callable
/// and sanitized metadata travel separately: only metadata enters the catalog
/// Iteration or JSON adapters. Native collection and item nodes preserve callable
/// identity, definition origin and privacy without a JSON or CEL round trip.
///
/// A catalog Iteration projects schemas once before the dispatch Iteration. All
/// dispatches share one arguments/router/result chain, regardless of tool count.
/// The returned Flow accepts `params.calls` and exports JSON `catalog` and ordered
/// `results`; it does not make a model request or export executable capabilities.
/// Existing upstream invocation/observation capability limits remain unchanged.
pub fn tool_uses_definition(
    tools: BTreeMap<String, FlowTool>,
) -> Result<WorkflowDefinition, DefinitionParseError> {
    let mut definition =
        WorkflowDefinition::from_json(include_str!("../workflows/tool-uses.json"))?;
    let bindings = definition
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "bindings")
        .expect("bundled tool-uses Flow has a bindings collector");
    bindings.config = json!({"names": tools.keys().collect::<Vec<_>>()});
    for (index, (name, tool)) in tools.into_iter().enumerate() {
        let id = format!("tool_{index}");
        definition.nodes.push(NodeDefinition {
            id: id.clone().into(),
            kind: "llm.tool".into(),
            config: json!({
                "name": name,
                "description": tool.description,
                "parameters": tool.parameters,
                "body": tool.body,
            }),
        });
        definition.edges.push(EdgeDefinition {
            exclusive: false,
            from_node: id.into(),
            from_output: "binding".into(),
            to_node: "bindings".into(),
            to_input: name,
        });
    }
    Ok(definition)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composition_materializes_each_tool_once_and_connects_native_bindings() {
        let body = WorkflowDefinition::from_json(
            &json!({"version":"2026-10-28", "nodes":[], "outputs":[]}).to_string(),
        )
        .unwrap();
        let tool = FlowTool {
            description: "Describe the child at this call site.".into(),
            parameters: BTreeMap::new(),
            body: body.clone(),
        };
        let definition = tool_uses_definition(BTreeMap::from([("lookup".into(), tool)])).unwrap();
        let producer = definition
            .nodes
            .iter()
            .find(|node| node.kind == "llm.tool")
            .unwrap();
        assert_eq!(producer.config["body"], json!(body));
        assert_eq!(
            producer.config["description"],
            "Describe the child at this call site."
        );
        assert_eq!(
            definition
                .nodes
                .iter()
                .filter(|node| node.kind == "llm.tool")
                .count(),
            1
        );
        assert!(definition.edges.iter().any(|edge| {
            edge.from_node == producer.id
                && edge.from_output == "binding"
                && edge.to_node.as_str() == "bindings"
                && edge.to_input == "lookup"
        }));
        let value = serde_json::to_value(&definition).unwrap();
        let nodes = value["nodes"].as_array().unwrap();
        let bindings = nodes.iter().find(|node| node["id"] == "bindings").unwrap();
        assert_eq!(bindings["config"], json!({"names":["lookup"]}));
        let dispatch = nodes.iter().find(|node| node["id"] == "dispatch").unwrap();
        let children = dispatch["config"]["body"]["nodes"].as_array().unwrap();
        let router = children
            .iter()
            .find(|node| node["id"] == "execute")
            .unwrap();
        assert_eq!(router["config"], json!({"mode":"execute"}));
        assert_eq!(
            children
                .iter()
                .filter(|node| node["kind"] == "builtin.router")
                .count(),
            2
        );
        assert_eq!(
            children
                .iter()
                .filter(|node| node["kind"] == "llm.tool_arguments")
                .count(),
            1
        );
        assert!(value.get("flow_arguments").is_none());
        assert_eq!(
            WorkflowDefinition::from_json(&value.to_string()).unwrap(),
            definition
        );
    }

    #[test]
    fn empty_composition_keeps_both_native_iterations() {
        let definition = tool_uses_definition(BTreeMap::new()).unwrap();
        assert_eq!(
            definition
                .nodes
                .iter()
                .filter(|node| node.kind == mf_runtime::ITERATION_KIND)
                .count(),
            2
        );
        assert!(definition.nodes.iter().all(|node| node.kind != "llm.tool"));
        assert!(definition.control_edges.iter().any(|edge| {
            edge.from_node.as_str() == "catalog" && edge.to_node.as_str() == "dispatch"
        }));
    }
}
