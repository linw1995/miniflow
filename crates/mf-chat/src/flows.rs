mf_compiler::embed_workflow!(pub mod conversation);

pub fn conversation_definition()
-> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError> {
    mf_runtime::WorkflowDefinition::from_json(include_str!(concat!(
        env!("OUT_DIR"),
        "/conversation.json"
    )))
}

pub fn title_definition() -> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError>
{
    mf_runtime::WorkflowDefinition::from_json(include_str!(concat!(env!("OUT_DIR"), "/title.json")))
}

pub fn compact_definition()
-> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError> {
    mf_runtime::WorkflowDefinition::from_json(include_str!(concat!(
        env!("OUT_DIR"),
        "/compact.json"
    )))
}

fn compact_run_definition()
-> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError> {
    mf_runtime::WorkflowDefinition::from_json(include_str!(concat!(
        env!("OUT_DIR"),
        "/compact-run.json"
    )))
}

fn compact_plan_definition()
-> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError> {
    mf_runtime::WorkflowDefinition::from_json(include_str!(concat!(
        env!("OUT_DIR"),
        "/compact-plan.json"
    )))
}

fn compact_summarize_definition()
-> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError> {
    mf_runtime::WorkflowDefinition::from_json(include_str!(concat!(
        env!("OUT_DIR"),
        "/compact-summarize.json"
    )))
}

mf_runtime::register_builtin_flow!("conversation", conversation_definition);
mf_runtime::register_builtin_flow!("chat-title", title_definition);
mf_runtime::register_builtin_flow!("chat-compact", compact_definition);
mf_runtime::register_builtin_flow!("chat-compact-run", compact_run_definition);
mf_runtime::register_builtin_flow!("chat-compact-plan", compact_plan_definition);
mf_runtime::register_builtin_flow!("chat-compact-summarize", compact_summarize_definition);
