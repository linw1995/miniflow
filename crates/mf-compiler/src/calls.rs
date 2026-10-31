use crate::{NodeDefinition, WorkflowCompileError, WorkflowDefinition};
use mf_runtime::CallConfig;
use snafu::{ResultExt, ensure};

pub fn body_definition(node: &NodeDefinition) -> Result<WorkflowDefinition, WorkflowCompileError> {
    let config: CallConfig = serde_json::from_value(node.config.clone()).context(
        crate::compiler::CallConfigurationSnafu {
            definition_id: node.id.clone(),
        },
    )?;
    ensure!(
        config.definition.is_some(),
        crate::compiler::InvalidCallSnafu {
            definition_id: node.id.clone(),
            message: "Flow reference has not been resolved"
        }
    );
    Ok(*config.definition.expect("resolved call has a body"))
}

pub fn contains_iteration_containers(
    definition: &WorkflowDefinition,
) -> Result<bool, WorkflowCompileError> {
    for node in &definition.nodes {
        if matches!(
            node.kind.as_str(),
            mf_runtime::LOOP_KIND | mf_runtime::ITERATION_KIND
        ) {
            return Ok(true);
        }
        if node.kind == mf_runtime::CALL_KIND
            && contains_iteration_containers(&body_definition(node)?)?
        {
            return Ok(true);
        }
        if let Some(body) = node.parameter_body(definition)
            && contains_iteration_containers(&body)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}
