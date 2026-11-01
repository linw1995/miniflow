mf_compiler::embed_workflow!(pub mod conversation);

pub fn title_definition() -> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError>
{
    mf_runtime::WorkflowDefinition::from_json(include_str!(concat!(env!("OUT_DIR"), "/title.json")))
}

mf_runtime::register_builtin_flow!("chat-title", title_definition);

// Compatibility name for existing chat profiles; ownership belongs to mfn-llm.
pub use mfn_llm::flows::agent_turn_definition;
mf_runtime::register_builtin_flow!("agent-turn", agent_turn_definition);
