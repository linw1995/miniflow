mf_compiler::embed_workflow!(pub mod conversation);

pub fn title_definition() -> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError>
{
    mf_runtime::WorkflowDefinition::from_json(include_str!(concat!(env!("OUT_DIR"), "/title.json")))
}

inventory::submit! {
    mf_runtime::BuiltinFlowRegistration { name: "mf-chat.chat-title", definition: title_definition }
}
