use mf_runtime::{NodeBuildError, NodeBusinessFailedSnafu, NodeExecutionError};
use rustix::io::Errno;
use snafu::{IntoError, Snafu};
use std::{error::Error, io, str::Utf8Error};

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum CodingError {
    #[snafu(display("configuration `{field}` requires {requirement}"))]
    InvalidConfig {
        field: &'static str,
        requirement: &'static str,
    },
    #[snafu(display("configuration `workspace_root`: {source}"))]
    WorkspaceTemplate {
        source: mf_runtime::ConfigTemplateError,
    },
    #[snafu(display("input `{field}` requires {requirement}"))]
    InvalidInput {
        field: &'static str,
        requirement: &'static str,
    },
    #[snafu(display("{operation}: {source}"))]
    Io {
        operation: &'static str,
        source: io::Error,
    },
    #[snafu(display("{operation}: {source}"))]
    ResolvePath {
        operation: &'static str,
        source: Errno,
    },
    #[snafu(display("file exceeds the {limit}-byte limit"))]
    FileTooLarge { limit: usize },
    #[snafu(display("file is not UTF-8: {source}"))]
    FileEncoding { source: Utf8Error },
    #[snafu(display("line {line} exceeds the {limit}-byte read page limit"))]
    LineTooLong { line: usize, limit: usize },
    #[snafu(display("invalid edit list: {source}"))]
    EditDecode { source: mf_runtime::TypeMismatch },
    #[snafu(display("edit {index} must {requirement}"))]
    EditMatch {
        index: usize,
        requirement: &'static str,
    },
    #[snafu(display("edit ranges overlap in the original file"))]
    OverlappingEdits,
    #[snafu(display("file changed during edit; read it again before retrying"))]
    FileChanged,
    #[snafu(display("another coding tool is modifying files in this directory"))]
    DirectoryBusy,
    #[snafu(display("input `timeout` requires a positive duration in seconds: {source}"))]
    ShellTimeout {
        source: std::time::TryFromFloatSecsError,
    },
    #[snafu(transparent)]
    Cancelled { source: mf_runtime::StreamError },
}

impl From<CodingError> for NodeBuildError {
    fn from(source: CodingError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

impl From<CodingError> for NodeExecutionError {
    fn from(source: CodingError) -> Self {
        let recoverable = match &source {
            CodingError::ResolvePath { source, .. } => matches!(
                *source,
                Errno::NOENT
                    | Errno::NOTDIR
                    | Errno::ISDIR
                    | Errno::LOOP
                    | Errno::EXIST
                    | Errno::NAMETOOLONG
                    | Errno::ACCESS
                    | Errno::PERM
                    | Errno::ROFS
            ),
            CodingError::InvalidInput { .. }
            | CodingError::FileTooLarge { .. }
            | CodingError::FileEncoding { .. }
            | CodingError::LineTooLong { .. }
            | CodingError::EditDecode { .. }
            | CodingError::EditMatch { .. }
            | CodingError::OverlappingEdits
            | CodingError::FileChanged
            | CodingError::DirectoryBusy
            | CodingError::ShellTimeout { .. } => true,
            _ => false,
        };
        let source = Box::<dyn Error + Send + Sync>::from(source);
        if recoverable {
            NodeBusinessFailedSnafu.into_error(source)
        } else {
            source.into()
        }
    }
}
