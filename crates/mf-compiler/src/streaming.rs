use crate::compiler::StreamConstructionSnafu;
use crate::{CompiledWorkflow, NodeRegistry, WorkflowCompileError, WorkflowExecutionError};
use mf_runtime::PreparedStream;
use snafu::ResultExt;

pub fn start_stream(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
    options: mf_runtime::StreamOptions,
) -> Result<mf_runtime::StreamInstance, WorkflowExecutionError> {
    let prepared = instantiate_stream(plan, registry).inspect_err(|error| {
        if let Some(observation) = &options.observation {
            observation.preparation_failed(error.to_string());
        }
    })?;
    Ok(mf_runtime::FlowRuntime::default().start_stream(prepared, options)?)
}

pub fn instantiate_stream(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<PreparedStream, WorkflowCompileError> {
    let (resolved, flow) = crate::compiler::prepare_compiled_plan(plan, registry)?;
    let execution = resolved.definition.execution.unwrap_or_default();
    flow.into_stream(execution).context(StreamConstructionSnafu)
}
