//! Fixed, provider-owned Flows. Tools are native input values, not graph templates.

use mf_runtime::{DefinitionParseError, WorkflowDefinition};

/// Project a native tool map into model-facing definitions.
/// The single catalog Iteration runs once before the Agent's model loop.
pub fn tool_catalog_definition() -> Result<WorkflowDefinition, DefinitionParseError> {
    WorkflowDefinition::from_json(include_str!(concat!(env!("OUT_DIR"), "/tool-catalog.json")))
}

/// Validate and execute a batch against the same native callable map.
pub fn tool_uses_definition() -> Result<WorkflowDefinition, DefinitionParseError> {
    WorkflowDefinition::from_json(include_str!(concat!(env!("OUT_DIR"), "/tool-uses.json")))
}

/// Assemble a request, forwarding the initial prefix directly when no suffix exists yet.
fn agent_request_definition() -> Result<WorkflowDefinition, DefinitionParseError> {
    WorkflowDefinition::from_json(include_str!(concat!(
        env!("OUT_DIR"),
        "/agent-request.json"
    )))
}

/// Make one provider request with the shared chat/streaming contract.
pub fn agent_turn_definition() -> Result<WorkflowDefinition, DefinitionParseError> {
    WorkflowDefinition::from_json(include_str!(concat!(env!("OUT_DIR"), "/agent-turn.json")))
}

/// Continue model requests with the fixed `params.tools` map while replies request tools.
/// Adding or replacing tools never changes this definition, its ports, or its body.
pub fn agent_definition() -> Result<WorkflowDefinition, DefinitionParseError> {
    WorkflowDefinition::from_json(include_str!(concat!(env!("OUT_DIR"), "/agent.json")))
}

mf_runtime::register_builtin_flow!("tool-catalog", tool_catalog_definition);
mf_runtime::register_builtin_flow!("tool-uses", tool_uses_definition);
mf_runtime::register_builtin_flow!("agent-request", agent_request_definition);
mf_runtime::register_builtin_flow!("agent-turn", agent_turn_definition);
mf_runtime::register_builtin_flow!("agent", agent_definition);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agent_is_one_fixed_definition_with_a_native_tool_collection_input() {
        let definition = agent_definition().unwrap();
        let params = definition
            .nodes
            .iter()
            .find(|node| node.id.as_str() == "params")
            .unwrap();
        assert_eq!(
            params.config["parameters"]["tools"]["type"],
            json!({"map":"flow"})
        );
        assert_eq!(
            params.config["parameters"]["tool_properties"]["type"],
            json!({"map":"object"})
        );
        assert!(params.config["parameters"].get("descriptions").is_none());
        let repeat = definition
            .nodes
            .iter()
            .find(|node| node.id.as_str() == "repeat")
            .unwrap();
        assert!(repeat.config.get("max_iterations").is_none());
        let loop_inputs = repeat.config["inputs"].as_object().unwrap();
        assert_eq!(loop_inputs.len(), 7);
        assert_eq!(loop_inputs["prefix"], json!({"list":"object"}));
        assert_eq!(loop_inputs["tools"], json!({"map":"flow"}));
        assert_eq!(loop_inputs["tool_properties"], json!({"map":"object"}));
        let variables = repeat.config["variables"].as_array().unwrap();
        assert_eq!(variables.len(), 5);
        assert!(
            variables
                .iter()
                .all(|variable| variable["name"] != "messages")
        );
        assert!(
            variables.iter().any(|variable| {
                variable["name"] == "usages" && variable["initial"] == json!([])
            })
        );
        let body: WorkflowDefinition =
            serde_json::from_value(repeat.config["body"].clone()).unwrap();
        for node_id in ["append_assistant", "append_tool_results"] {
            let node = body
                .nodes
                .iter()
                .find(|node| node.id.as_str() == node_id)
                .unwrap();
            assert!(node.config["code"].get("messages").is_none(), "{node_id}");
            assert!(node.config["inputs"].get("messages").is_none(), "{node_id}");
            assert!(
                node.config["inputs"].get("assistant").is_none(),
                "{node_id}"
            );
        }
        assert!(body.edges.iter().all(|edge| {
            !(edge.from_node.as_str() == "request" && edge.to_node.as_str() == "append_assistant")
        }));
        let serialized = serde_json::to_string(&definition).unwrap();
        for synthetic in [
            "flow_0",
            "tool_0",
            "definition_0",
            "binding_0",
            "flow_inputs",
        ] {
            assert!(!serialized.contains(synthetic), "{synthetic}");
        }
        assert_eq!(definition.outputs.len(), 8);
        assert!(
            definition
                .outputs
                .iter()
                .any(|output| output.name == "tool_output"
                    && output.node.as_str() == "repeat"
                    && output.port == "tool_output"
                    && output.optional)
        );
        assert!(
            definition
                .nodes
                .iter()
                .any(|node| node.kind == mf_runtime::LOOP_KIND)
        );
    }

    #[test]
    fn catalog_uses_one_projection_inside_a_native_iteration() {
        let definition = tool_catalog_definition().unwrap();
        let project = definition
            .nodes
            .iter()
            .find(|node| node.kind == mf_runtime::ITERATION_KIND)
            .unwrap();
        let body: WorkflowDefinition =
            serde_json::from_value(project.config["body"].clone()).unwrap();
        assert_eq!(
            body.nodes
                .iter()
                .filter(|node| node.kind == "builtin.code")
                .count(),
            1
        );
        assert!(
            body.nodes
                .iter()
                .all(|node| node.kind != "llm.tool_definition")
        );
        assert_eq!(
            definition
                .outputs
                .iter()
                .map(|output| output.name.as_ref())
                .collect::<Vec<_>>(),
            ["tools"]
        );
    }

    #[test]
    fn definitions_bind_workspace_dependency_versions() {
        let expected = format!("={}", env!("CARGO_PKG_VERSION"));
        for definition in [
            tool_catalog_definition().unwrap(),
            tool_uses_definition().unwrap(),
            agent_request_definition().unwrap(),
            agent_turn_definition().unwrap(),
            agent_definition().unwrap(),
        ] {
            assert!(!definition.dependencies.is_empty());
            for dependency in definition.dependencies.values() {
                assert_eq!(dependency.version.as_deref(), Some(expected.as_str()));
            }
        }
        for source in [
            include_str!("../workflows/tool-catalog.json"),
            include_str!("../workflows/tool-uses.json"),
            include_str!("../workflows/agent-request.json"),
            include_str!("../workflows/agent-turn.json"),
            include_str!("../workflows/agent.json"),
        ] {
            assert!(!source.contains("=0.1.0"));
        }
    }

    #[test]
    fn provider_agent_turn_keeps_its_streaming_answer() {
        let turn = agent_turn_definition().unwrap();
        assert!(
            turn.outputs
                .iter()
                .any(|output| output.name == "answer" && output.port == "delta" && output.optional)
        );
    }
}
