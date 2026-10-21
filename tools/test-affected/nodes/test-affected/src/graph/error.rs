use snafu::Snafu;
use std::{error::Error as StdError, path::PathBuf, time::SystemTimeError};

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("coverage graph {path:?} does not exist"))]
    Missing { path: PathBuf },
    #[snafu(display("coverage graph path {path:?} must be UTF-8"))]
    Path { path: PathBuf },
    #[snafu(display("could not connect to coverage graph {path:?}: {source}"))]
    Connect {
        path: PathBuf,
        source: diesel::ConnectionError,
    },
    #[snafu(display("could not migrate coverage graph {path:?}: {source}"))]
    Migration {
        path: PathBuf,
        source: Box<dyn StdError + Send + Sync>,
    },
    #[snafu(display("could not {operation}: {source}"))]
    Database {
        operation: &'static str,
        source: diesel::result::Error,
    },
    #[snafu(transparent)]
    Query { source: diesel::result::Error },
    #[snafu(display("coverage graph already indexes commit {commit_sha}"))]
    DuplicateCommit { commit_sha: String },
    #[snafu(display("invalid coverage manifest {field}: {reason}"))]
    InvalidManifest {
        field: &'static str,
        reason: &'static str,
    },
    #[snafu(display("could not {action} at {path:?}: {source}"))]
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("could not {action}: {source}"))]
    Json {
        action: &'static str,
        source: serde_json::Error,
    },
    #[snafu(display("invalid lcov export {path:?}: malformed {record} record"))]
    Lcov { path: PathBuf, record: &'static str },
    #[snafu(display("invalid lcov export {path:?}: non-numeric {record} record: {source}"))]
    LcovNumber {
        path: PathBuf,
        record: &'static str,
        source: std::num::ParseIntError,
    },
    #[snafu(display("invalid coverage query range {start}..={end}"))]
    InvalidRange { start: u32, end: u32 },
    #[snafu(display("could not read UTC time: {source}"))]
    Clock { source: SystemTimeError },
    #[snafu(display("UTC timestamp is out of range: {source}"))]
    Timestamp { source: std::num::TryFromIntError },
}
