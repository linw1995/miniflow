use crate::error::*;
use mf_runtime::{ExecutionContext, FutureExecutionError, NodeExecutionError, NodeValue, ValueRef};
use serde_json::{Value, json};
use snafu::{ResultExt, ensure};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(NodeValue)]
pub struct EvidenceOutput {
    pub result: ValueRef,
    pub deadline: u64,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

pub struct Budget {
    pub deadline: u64,
    pub instant: Instant,
}

impl Budget {
    pub fn new(deadline: Option<u64>, timeout_ms: u64) -> Result<Self, Error> {
        ensure!(
            (1..=600_000).contains(&timeout_ms),
            InvalidSnafu {
                message: "timeout_ms must be in 1..=600000"
            }
        );
        let now = now_ms();
        let deadline = deadline.unwrap_or_else(|| now.saturating_add(60_000));
        let remaining = deadline.saturating_sub(now).min(timeout_ms);
        Ok(Self {
            deadline,
            instant: Instant::now() + Duration::from_millis(remaining),
        })
    }

    pub fn check(&self, context: &ExecutionContext) -> Result<(), Error> {
        if let Some(source) = context.cancellation().failure() {
            return Err(source).context(CancelledSnafu);
        }
        ensure!(
            Instant::now() < self.instant && now_ms() < self.deadline,
            LimitSnafu {
                message: "evidence deadline exceeded"
            }
        );
        Ok(())
    }

    pub fn outcome(
        &self,
        result: Result<Value, Error>,
        required: bool,
    ) -> Result<EvidenceOutput, NodeExecutionError> {
        let result = result.and_then(|value| {
            ensure!(
                bounded_shape(&value, 0, &mut 4096),
                LimitSnafu {
                    message: "evidence JSON exceeds downstream structural limits"
                }
            );
            Ok(value)
        });
        let mut value = match result {
            Ok(value) => json!({"available":true,"value":value,"reason":null}),
            Err(error) => {
                let reason = match &error {
                    Error::Cancelled { .. } | Error::Invalid { .. } | Error::Cleanup { .. } => {
                        return Err(error.into());
                    }
                    Error::Future {
                        source: FutureExecutionError::DeadlineExceeded,
                    }
                    | Error::Limit { .. } => "limit",
                    Error::Future { .. } => return Err(error.into()),
                    Error::Response { .. } => "response",
                    Error::Archive { .. } | Error::Json { .. } => "invalid_data",
                    _ => "unavailable",
                };
                if required {
                    return Err(error.into());
                }
                json!({"available":false,"value":null,"reason":reason})
            }
        };
        value["remaining_ms"] = json!(self.deadline.saturating_sub(now_ms()));
        Ok(EvidenceOutput {
            result: value.into(),
            deadline: self.deadline,
        })
    }
}

// Leave room for the envelope and other Code inputs within its shared traversal budget.
fn bounded_shape(value: &Value, depth: usize, remaining: &mut usize) -> bool {
    if depth > 10 {
        return false;
    }
    let length = match value {
        Value::Array(v) => v.len(),
        Value::Object(v) => v.len(),
        _ => 0,
    };
    let Some(next) = remaining.checked_sub(length) else {
        return false;
    };
    *remaining = next;
    match value {
        Value::Array(values) => values
            .iter()
            .all(|v| bounded_shape(v, depth + 1, remaining)),
        Value::Object(values) => values
            .values()
            .all(|v| bounded_shape(v, depth + 1, remaining)),
        _ => true,
    }
}
