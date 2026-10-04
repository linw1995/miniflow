use crate::compiler::{
    FlowConstructionSnafu, InvalidStreamSnafu, NonCanonicalPlanOrderSnafu, StreamConstructionSnafu,
};
use crate::{CompiledWorkflow, Flow, NodeRegistry, WorkflowCompileError};
use mf_runtime::PreparedStream;
use snafu::{OptionExt, ResultExt, ensure};

pub fn start_stream(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
    options: mf_runtime::StreamOptions,
) -> Result<mf_runtime::StreamInstance, mf_runtime::StreamError> {
    let prepared = instantiate_stream(plan, registry)
        .inspect_err(|error| {
            if let Some(observation) = &options.observation {
                observation.preparation_failed(error.to_string());
            }
        })
        .boxed()
        .context(mf_runtime::StreamCompilationSnafu)?;
    prepared.start_with_options(options)
}

pub fn instantiate_stream(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<PreparedStream, WorkflowCompileError> {
    let execution = plan
        .definition
        .execution
        .clone()
        .context(InvalidStreamSnafu {
            message: "workflow does not declare streaming execution",
        })?;
    let (nodes, order) = crate::compiler::prepare_definition(&plan.definition, registry)?;
    ensure!(order == plan.execution_order, NonCanonicalPlanOrderSnafu);
    Flow::prepare(
        nodes,
        plan.definition.edges.clone(),
        order,
        plan.definition.outputs.clone(),
    )
    .and_then(|flow| flow.with_control_edges(plan.definition.control_edges.clone()))
    .context(FlowConstructionSnafu)?
    .into_stream(execution)
    .context(StreamConstructionSnafu)
}
