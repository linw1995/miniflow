use snafu::Snafu;
use std::{error::Error, io, path::PathBuf};

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum ChatError {
    #[snafu(display("could not parse the application conversation Flow: {source}"))]
    ConversationDefinition {
        source: mf_runtime::DefinitionParseError,
    },
    #[snafu(transparent)]
    FlowResolution {
        source: mf_compiler::FlowResolutionError,
    },
    #[snafu(transparent)]
    Stream { source: mf_runtime::StreamError },
    #[snafu(display("could not deliver conversation preview: {source}"))]
    PreviewDelivery {
        source: Box<dyn Error + Send + Sync>,
    },
    #[snafu(display("conversation workflow failed: {message}"))]
    WorkflowFailure { message: String },
    #[snafu(transparent)]
    Inputs {
        source: mf_runtime::WorkflowInputError,
    },
    #[snafu(display("could not capture workflow result: {message}"))]
    Capture { message: String },
    #[snafu(display("model reply exceeds the {limit}-byte limit"))]
    RecoveryTooLarge { limit: usize },
    #[snafu(transparent)]
    Manifest {
        source: mf_runtime::WorkflowManifestError,
    },
    #[snafu(transparent)]
    Description {
        source: mf_compiler::DescriptionError,
    },
    #[snafu(display("chat workflow profile {name:?} does not exist"))]
    ProfileMissing { name: String },
    #[snafu(display("chat host is closed"))]
    Closed,
    #[snafu(display("chat host reached its {limit}-workflow capacity"))]
    Capacity { limit: usize },
    #[snafu(display("could not prepare the conversation backend: {source}"))]
    BackendBuild {
        #[snafu(source(from(mf_runtime::NodeBuildError, Box::new)))]
        source: Box<mf_runtime::NodeBuildError>,
    },
    #[snafu(transparent)]
    FlowBuild { source: mf_compiler::FlowBuildError },
    #[snafu(transparent)]
    StreamBuild {
        source: mf_compiler::StreamBuildError,
    },
    #[snafu(display("conversation backend failed: {source}"))]
    Backend {
        outcome: crate::contract::TurnExit,
        reply: Option<Box<crate::ConversationResult>>,
        source: Box<dyn Error + Send + Sync>,
    },
    #[snafu(display("could not start chat worker: {source}"))]
    WorkerSpawn { source: io::Error },
    #[snafu(display("could not receive chat worker result: {source}"))]
    WorkerReceive { source: std::sync::mpsc::RecvError },
    #[snafu(display("chat worker {id:?} panicked"))]
    WorkerPanic { id: String },
    #[snafu(display("chat invocation {invocation:?} failed: {source}; cleanup: {cleanup:?}"))]
    Execution {
        invocation: crate::Invocation,
        cleanup: Option<Box<ChatError>>,
        source: Box<ChatError>,
    },
    #[snafu(display("chat response for {:?} is not committed: {source}", pending.completion.invocation))]
    Uncommitted {
        pending: Box<crate::UncommittedCompletion>,
        source: Box<ChatError>,
    },
    #[snafu(transparent)]
    Plan {
        #[snafu(source(from(mf_compiler::PlanError, Box::new)))]
        source: Box<mf_compiler::PlanError>,
    },
    #[snafu(display("chat session {id:?} is archived"))]
    SessionArchived { id: String },
    #[snafu(display("chat session {id:?} already has a running turn"))]
    SessionBusy { id: String },
    #[snafu(display("chat turn {id:?} does not exist"))]
    TurnMissing { id: String },
    #[snafu(display("model call {id:?} does not exist"))]
    CallMissing { id: String },
    #[snafu(display("model call {id:?} has a conflicting identity, state, or response"))]
    Conflict { id: String },
    #[snafu(display("could not {operation}: {source}"))]
    Integer {
        operation: &'static str,
        source: std::num::TryFromIntError,
    },
    #[snafu(display("invalid chat {field}: {reason}"))]
    Invalid {
        field: &'static str,
        reason: &'static str,
    },
    #[snafu(display("chat session {id:?} does not exist"))]
    SessionMissing { id: String },
    #[snafu(display("history exceeds the {limit}-byte bound"))]
    HistoryTooLarge { limit: usize },
    #[snafu(display("could not {operation}: integer range exceeded"))]
    Arithmetic { operation: &'static str },
    #[snafu(display("could not read UTC time: {source}"))]
    Clock { source: std::time::SystemTimeError },
    #[snafu(display("UTC timestamp is out of range: {source}"))]
    Timestamp { source: std::num::TryFromIntError },
    #[snafu(display("could not convert chat JSON values: {source}"))]
    Json { source: serde_json::Error },
    #[snafu(transparent)]
    Registry {
        source: mf_runtime::NodeRegistryError,
    },
    #[snafu(transparent)]
    Compile {
        #[snafu(source(from(mf_compiler::WorkflowCompileError, Box::new)))]
        source: Box<mf_compiler::WorkflowCompileError>,
    },
    #[snafu(transparent)]
    Run {
        #[snafu(source(from(mf_runtime::WorkflowRunError, Box::new)))]
        source: Box<mf_runtime::WorkflowRunError>,
    },
    #[snafu(transparent)]
    Encode {
        source: mf_runtime::OutputEncodeError,
    },
    #[snafu(transparent)]
    Decode {
        source: mf_runtime::InputDecodeError,
    },
    #[snafu(display("could not {operation} at {path:?}: {source}"))]
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    #[snafu(display("database path {path:?} must have a UTF-8 filename"))]
    Path { path: PathBuf },
    #[snafu(display("chat database {path:?} already has an active owner"))]
    Busy { path: PathBuf },
    #[snafu(display("could not connect to chat database {path:?}: {source}"))]
    Connect {
        path: PathBuf,
        source: diesel::ConnectionError,
    },
    #[snafu(display("could not {operation}: {source}"))]
    Database {
        operation: &'static str,
        source: diesel::result::Error,
    },
    #[snafu(display("could not migrate chat database: {source}"))]
    Migration {
        source: Box<dyn Error + Send + Sync>,
    },
    #[snafu(transparent)]
    Query { source: diesel::result::Error },
    #[snafu(transparent)]
    Node {
        source: mf_runtime::NodeExecutionError,
    },
}

impl From<ChatError> for mf_runtime::NodeExecutionError {
    fn from(source: ChatError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

impl From<ChatError> for mf_runtime::NodeBuildError {
    fn from(source: ChatError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}
