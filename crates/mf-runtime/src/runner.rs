use crate::NodeExecutionError;
use crate::definition::DefinitionId;
use snafu::Snafu;

#[derive(Debug, Snafu)]
pub enum WorkflowRunError {
    #[snafu(
        display("live output delivery failed: {source}"),
        visibility(pub(super))
    )]
    LiveOutput { source: crate::StreamError },
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn live_delivery_failure_preserves_its_runtime_source() {
        let error = WorkflowRunError::LiveOutput {
            source: crate::StreamError::Execution {
                message: "transport disconnected".into(),
            },
        };
        assert!(error.to_string().contains("live output delivery failed"));
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<crate::StreamError>()
            .unwrap();
        assert!(
            matches!(source, crate::StreamError::Execution { message } if message == "transport disconnected")
        );
    }
}
