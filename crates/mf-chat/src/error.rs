use snafu::Snafu;
use std::{error::Error, io, path::PathBuf};

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum ChatError {
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
