mf_compiler::embed_workflow!(pub mod conversation);

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

fn compact_prepare_definition()
-> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError> {
    mf_runtime::WorkflowDefinition::from_json(include_str!(concat!(
        env!("OUT_DIR"),
        "/compact-prepare.json"
    )))
}

fn compact_summarize_definition()
-> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError> {
    mf_runtime::WorkflowDefinition::from_json(include_str!(concat!(
        env!("OUT_DIR"),
        "/compact-summarize.json"
    )))
}

mf_runtime::register_builtin_flow!("chat-title", title_definition);
mf_runtime::register_builtin_flow!("chat-compact", compact_definition);
mf_runtime::register_builtin_flow!("chat-compact-run", compact_run_definition);
mf_runtime::register_builtin_flow!("chat-compact-prepare", compact_prepare_definition);
mf_runtime::register_builtin_flow!("chat-compact-summarize", compact_summarize_definition);
