mf_compiler::embed_workflow!(pub mod conversation);

pub fn title_definition() -> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError>
{
    let mut definition =
        mf_runtime::WorkflowDefinition::from_json(include_str!("../workflows/title.json"))?;
    for dependency in definition.dependencies.values_mut() {
        dependency.version = Some(format!("={}", env!("CARGO_PKG_VERSION")));
    }
    Ok(definition)
}

inventory::submit! {
    mf_runtime::BuiltinFlowRegistration { name: "chat-title", definition: title_definition }
}
