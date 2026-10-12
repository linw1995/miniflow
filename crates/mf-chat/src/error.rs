use snafu::Snafu;
use std::{error::Error, io, path::PathBuf};

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum ChatError {
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
