use crate::NodeExecutionError;
use crate::definition::DefinitionId;
use snafu::Snafu;

#[derive(Debug, Snafu)]
pub enum WorkflowRunError {
    #[snafu(
        display(
            "node `{definition_id}`: scheduled-step budget of {} exhausted",
            crate::MAX_SCHEDULED_STEPS
        ),
        visibility(pub)
    )]
    StepBudget { definition_id: DefinitionId },
    #[snafu(
        display("node `{definition_id}` requires successful node `{source_node}`: {source}"),
        visibility(pub)
    )]
    SuccessDependency {
        definition_id: DefinitionId,
        source_node: String,
        source: NodeExecutionError,
    },
    #[snafu(transparent)]
    NodeTarget { source: crate::NodeTargetError },
    #[snafu(transparent)]
    WorkflowInputs { source: crate::WorkflowInputError },
    #[snafu(transparent)]
    WorkerPool { source: crate::WorkerPoolError },
    #[snafu(
        display("node `{definition_id}` dependency `{input}`: {source}"),
        visibility(pub)
    )]
    Dependency {
        definition_id: DefinitionId,
        input: String,
        source: NodeExecutionError,
    },
    #[snafu(
        display("node `{definition_id}`: workflow output `{output}`: {source}"),
        visibility(pub)
    )]
    OutputSelection {
        definition_id: DefinitionId,
        output: String,
        source: NodeExecutionError,
    },
    #[snafu(
        display("node `{definition_id}` input `{input}`: {source}"),
        visibility(pub)
    )]
    InputType {
        definition_id: DefinitionId,
        input: String,
        source: crate::TypeMismatch,
    },
    #[snafu(
        display("node `{definition_id}` output `{output}`: {source}"),
        visibility(pub)
    )]
    OutputType {
        definition_id: DefinitionId,
        output: String,
        source: crate::TypeMismatch,
    },
    #[snafu(display("node `{definition_id}`: {message}"), visibility(pub))]
    InvalidOutput {
        definition_id: DefinitionId,
        message: String,
    },
    #[snafu(display("node `{definition_id}`: {message}"), visibility(pub))]
    Context {
        definition_id: DefinitionId,
        message: String,
    },
    #[snafu(display("node `{definition_id}` failed: {source}"), visibility(pub))]
    NodeExecution {
        source: NodeExecutionError,
        definition_id: DefinitionId,
    },
}

impl WorkflowRunError {
    pub(crate) fn is_output_error(&self) -> bool {
        matches!(self, Self::OutputType { .. } | Self::InvalidOutput { .. })
    }
}

impl WorkflowRunError {
    /// Terminal runtime controls must not become recoverable tool failures.
    pub fn is_terminal_control(&self) -> bool {
        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(self);
        while let Some(error) = source {
            if matches!(error.downcast_ref::<Self>(), Some(Self::StepBudget { .. })) {
                return true;
            }
            if matches!(
                error.downcast_ref::<crate::CallDispatchError>(),
                Some(
                    crate::CallDispatchError::DepthLimit
                        | crate::CallDispatchError::IdentityLimit
                        | crate::CallDispatchError::DeadlineExceeded
                        | crate::CallDispatchError::Cancelled { .. }
                )
            ) {
                return true;
            }
            #[cfg(feature = "async")]
            if matches!(
                error.downcast_ref::<crate::FutureExecutionError>(),
                Some(
                    crate::FutureExecutionError::DeadlineExceeded
                        | crate::FutureExecutionError::Cancelled { .. }
                )
            ) {
                return true;
            }
            source = error.source();
        }
        false
    }
}
