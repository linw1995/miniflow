use crate::compiler::StreamConstructionSnafu;
use crate::{CompiledWorkflow, NodeRegistry, WorkflowCompileError, WorkflowExecutionError};
use mf_runtime::PreparedStream;
use snafu::ResultExt;

pub fn callable_interface(
    plan: &mf_runtime::StreamPlan,
    selections: &[mf_runtime::WorkflowOutputDefinition],
    input_metadata: mf_runtime::SubgraphInputMetadata,
) -> mf_runtime::SubgraphInterface {
    let mut interface = mf_runtime::SubgraphInterface {
        input_metadata,
        ..Default::default()
    };
    let ports = crate::construction::stream_output_messages(plan);
    let sparse = plan.selected_domains().filter(|m| *m != 0).count() > 1;
    for selection in selections {
        let (port, message) = ports[&(selection.node.as_str(), selection.port.as_ref())];
        let terminal = message == 0;
        let spec = mf_runtime::PortSpec::owned(
            selection.name.as_ref(),
            port.value_type.clone(),
            !selection.optional && (terminal || !sparse),
        );
        if terminal {
            interface.outputs.push(spec);
        } else {
            interface.stream_outputs.push(spec);
        }
    }
    interface
}

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
    let execution = resolved.graph.definition.execution.unwrap_or_default();
    flow.into_stream(execution).context(StreamConstructionSnafu)
}
