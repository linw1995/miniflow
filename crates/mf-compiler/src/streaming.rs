use crate::{CompiledWorkflow, Flow, NodeRegistry, WorkflowCompileError, WorkflowDefinition};
use mf_runtime::{NodeDefinition, PreparedStream, STREAM_INPUT_ID};
use std::borrow::Cow;

pub fn start_stream(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
    options: mf_runtime::StreamOptions,
) -> Result<mf_runtime::StreamInstance, mf_runtime::StreamError> {
    let prepared = instantiate_stream(plan, registry).map_err(|error| {
        mf_runtime::StreamError::Preparation {
            message: error.to_string(),
        }
    })?;
    prepared.start_with_options(options)
}

pub fn expanded_definition(
    definition: &WorkflowDefinition,
) -> Result<Cow<'_, WorkflowDefinition>, WorkflowCompileError> {
    definition
        .validate_execution()
        .map_err(|message| WorkflowCompileError::InvalidStream { message })?;
    if definition.nodes.iter().any(|node| {
        node.kind == STREAM_INPUT_ID
            || (definition.execution.is_some() && node.id.as_str() == STREAM_INPUT_ID)
    }) {
        return Err(WorkflowCompileError::InvalidStream {
            message: "%input is reserved for the engine input source".into(),
        });
    }
    if definition.execution.is_none() {
        return Ok(Cow::Borrowed(definition));
    }
    let mut expanded = definition.clone();
    expanded.nodes.insert(
        0,
        NodeDefinition {
            id: STREAM_INPUT_ID.into(),
            kind: STREAM_INPUT_ID.into(),
            config: serde_json::json!({}),
            loop_definition: None,
        },
    );
    Ok(Cow::Owned(expanded))
}

pub fn instantiate_stream(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<PreparedStream, WorkflowCompileError> {
    let execution =
        plan.definition
            .execution
            .clone()
            .ok_or_else(|| WorkflowCompileError::InvalidStream {
                message: "workflow does not declare streaming execution".into(),
            })?;
    let (nodes, order) = crate::compiler::prepare_definition(&plan.definition, registry)?;
    if order
        .iter()
        .filter(|id| id.as_str() != STREAM_INPUT_ID)
        .ne(plan.execution_order.iter())
    {
        return Err(WorkflowCompileError::NonCanonicalPlanOrder);
    }
    Flow::prepare(
        nodes,
        plan.definition.edges.clone(),
        order,
        plan.definition.outputs.clone(),
    )
    .and_then(|flow| flow.with_control_edges(plan.definition.control_edges.clone()))
    .map_err(|source| WorkflowCompileError::FlowConstruction { source })?
    .into_stream(execution)
    .map_err(|error| WorkflowCompileError::InvalidStream {
        message: error.to_string(),
    })
}
