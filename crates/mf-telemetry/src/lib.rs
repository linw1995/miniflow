//! Observation contracts without provider installation or network initialization.

pub mod capture;
pub mod description;
pub mod event;
pub mod events;
pub mod identity;
pub mod iteration;
pub mod observation;
#[cfg(feature = "otlp")]
pub mod otlp;
#[cfg(feature = "snapshot")]
pub mod snapshot;
pub mod stream;
pub mod transport;
pub mod wire;

pub const SNAPSHOT_CAPTURE_ENV: &str = "MF_CAPTURE_SNAPSHOTS";

use snafu::{Snafu, ensure};

pub const INSTRUMENTATION_SCOPE: &str = "mf.workflow";
pub const EVENT_SCHEMA_VERSION: i64 = 7;
pub const STREAM_EVENT_SCHEMA_VERSION: i64 = 8;
/// Extended finite and stream contracts distinguish runtime Flow frames from definitions.
pub const FLOW_EVENT_SCHEMA_VERSION: i64 = 9;
pub const FLOW_STREAM_EVENT_SCHEMA_VERSION: i64 = 10;

pub use event::FlowOrigin;
/// Combined static nesting bound for ordinary calls and Loop scopes.
pub const MAX_SCOPE_DEPTH: usize = 20;
pub const MAX_LOOP_DEPTH: usize = 4;
pub const MAX_LOOP_ITERATIONS: u16 = 1000;

#[derive(Debug, Snafu)]
pub enum ContractError {
    #[snafu(display("invalid observation contract: {message}"), visibility(pub))]
    Invalid { message: String },
    #[snafu(display(
        "observed Flow origin belongs to workflow {origin_workflow}, not {owning_workflow}; cross-artifact observation is unsupported"
    ))]
    UnsupportedFlowOrigin {
        owning_workflow: identity::WorkflowId,
        origin_workflow: identity::WorkflowId,
    },
    #[snafu(display(
        "observed Flow invocation inside an iteration item does not support target {definition:?} declaring nested state/item scopes"
    ))]
    UnsupportedFlowIterationScope { definition: FlowOrigin },
    #[snafu(display("invalid run ID: {source}"))]
    RunId { source: uuid::Error },
    #[snafu(display("invalid trace ID: {source}"))]
    TraceId { source: std::num::ParseIntError },
    #[snafu(display("invalid span ID: {source}"))]
    SpanId { source: std::num::ParseIntError },
    #[snafu(display("invalid observation JSON: {source}"), context(false))]
    Json { source: serde_json::Error },
}

fn invalid(message: impl Into<String>) -> ContractError {
    InvalidSnafu { message }.build()
}

fn require(condition: bool, message: impl Into<String>) -> Result<(), ContractError> {
    ensure!(condition, InvalidSnafu { message });
    Ok(())
}

/// Counts and monotonic offsets use OTel's nonnegative signed integer range.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(try_from = "i64", into = "i64")]
pub struct Count(i64);

impl Count {
    pub const ZERO: Self = Self(0);

    pub fn get(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for Count {
    type Error = ContractError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        require(value >= 0, "counts and offsets must be nonnegative")?;
        Ok(Self(value))
    }
}

impl From<Count> for i64 {
    fn from(value: Count) -> Self {
        value.0
    }
}
