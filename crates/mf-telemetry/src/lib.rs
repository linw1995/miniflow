//! Observation contracts without provider installation or network initialization.

pub mod description;
pub mod event;
pub mod identity;
pub mod observation;
#[cfg(feature = "otlp")]
pub mod otlp;
#[cfg(feature = "snapshot")]
pub mod snapshot;
pub mod stream;
pub mod wire;

pub const SNAPSHOT_CAPTURE_ENV: &str = "MF_CAPTURE_SNAPSHOTS";

use snafu::Snafu;

pub const INSTRUMENTATION_SCOPE: &str = "mf.workflow";
pub const EVENT_SCHEMA_VERSION: i64 = 1;
pub const LOOP_EVENT_SCHEMA_VERSION: i64 = 2;
pub const LEGACY_STREAM_EVENT_SCHEMA_VERSION: i64 = 3;
pub const STREAM_EVENT_SCHEMA_VERSION: i64 = 4;
pub const MAX_LOOP_DEPTH: usize = 4;
pub const MAX_LOOP_ITERATIONS: u16 = 1000;
pub const MAX_LOOP_SCHEDULED_STEPS: i64 = 10_000;

#[derive(Debug, Snafu)]
pub enum ContractError {
    #[snafu(display("invalid observation contract: {message}"))]
    Invalid { message: String },
    #[snafu(display("invalid observation JSON: {source}"), context(false))]
    Json { source: serde_json::Error },
}

fn invalid(message: impl Into<String>) -> ContractError {
    ContractError::Invalid {
        message: message.into(),
    }
}

fn require(condition: bool, message: impl Into<String>) -> Result<(), ContractError> {
    if condition {
        Ok(())
    } else {
        Err(invalid(message))
    }
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

pub fn maximum_event_count(node_count: Count) -> Result<Count, ContractError> {
    node_count
        .0
        .checked_mul(2)
        .and_then(|n| n.checked_add(2))
        .map(Count)
        .ok_or_else(|| invalid("node count exceeds lifecycle sequence capacity"))
}

pub fn maximum_loop_event_count() -> Count {
    Count(4 * MAX_LOOP_SCHEDULED_STEPS + 4)
}
