pub fn definition() -> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError> {
    let mut definition =
        mf_runtime::WorkflowDefinition::from_json(include_str!("../workflows/agent.json"))?;
    for dependency in definition.dependencies.values_mut() {
        dependency.version = Some(format!("={}", env!("CARGO_PKG_VERSION")));
    }
    let rounds = definition
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "rounds")
        .expect("built-in agent declares its round loop");
    rounds
        .loop_definition
        .as_mut()
        .expect("rounds is a Loop")
        .max_iterations = mf_runtime::MAX_LOOP_ITERATIONS;
    let limits = definition
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "limits")
        .expect("built-in agent validates its budgets");
    limits.config["code"]["valid"] = format!(
        "max_rounds >= 1 && max_rounds <= {} && max_tool_calls >= 1 && max_tool_calls <= {}",
        mf_runtime::MAX_LOOP_ITERATIONS,
        crate::tools::MAX_TOOL_CALLS
    )
    .into();
    Ok(definition)
}

mf_runtime::register_builtin_flow!("agent", definition);
