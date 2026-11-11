//! Fixed, provider-owned Flows. Tools are native input values, not graph templates.

use mf_runtime::{DefinitionParseError, WorkflowDefinition};

/// Project a native tool map into model-facing definitions.
/// The single catalog Iteration runs once before the Agent's model loop.
pub fn tool_catalog_definition() -> Result<WorkflowDefinition, DefinitionParseError> {
    WorkflowDefinition::from_json(include_str!("../workflows/tool-catalog.json"))
}

/// Validate and execute a batch against the same native callable map.
pub fn tool_uses_definition() -> Result<WorkflowDefinition, DefinitionParseError> {
    WorkflowDefinition::from_json(include_str!("../workflows/tool-uses.json"))
}

/// Make one provider request with the shared chat/streaming contract.
pub fn agent_turn_definition() -> Result<WorkflowDefinition, DefinitionParseError> {
    WorkflowDefinition::from_json(include_str!("../workflows/agent-turn.json"))
}

/// Run the bounded Agent with `params.tools` and caller-owned `params.descriptions`.
/// Adding or replacing tools never changes this definition, its ports, or its body.
pub fn agent_definition() -> Result<WorkflowDefinition, DefinitionParseError> {
    WorkflowDefinition::from_json(include_str!("../workflows/agent.json"))
}

mf_runtime::register_builtin_flow!("tool-catalog", tool_catalog_definition);
mf_runtime::register_builtin_flow!("tool-uses", tool_uses_definition);
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
        assert_eq!(definition.outputs.len(), 5);
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
                .filter(|node| node.kind == "llm.tool_definition")
                .count(),
            1
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
    fn provider_agent_turn_keeps_its_streaming_answer() {
        let turn = agent_turn_definition().unwrap();
        assert!(
            turn.outputs
                .iter()
                .any(|output| output.name == "answer" && output.port == "delta" && output.optional)
        );
    }
}
