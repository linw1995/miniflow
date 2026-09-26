//! Observation contracts without provider installation or network initialization.

pub mod description;
pub mod event;
pub mod identity;
pub mod wire;

use snafu::Snafu;

pub const INSTRUMENTATION_SCOPE: &str = "mf.workflow";
pub const EVENT_SCHEMA_VERSION: i64 = 1;
pub const DESCRIPTION_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Snafu)]
pub enum ContractError {
    #[snafu(display("invalid observation contract: {message}"))]
    Invalid { message: String },
    #[snafu(display("invalid observation JSON: {source}"))]
    Json { source: serde_json::Error },
}

impl From<serde_json::Error> for ContractError {
    fn from(source: serde_json::Error) -> Self {
        Self::Json { source }
    }
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
