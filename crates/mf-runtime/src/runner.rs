use crate::NodeExecutionError;
use crate::definition::DefinitionId;
use snafu::Snafu;

#[derive(Debug, Snafu)]
pub enum WorkflowRunError {
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
