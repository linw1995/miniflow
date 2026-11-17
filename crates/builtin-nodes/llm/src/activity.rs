//! Non-authoritative tool lifecycle events for live conversation presentation.

use crate::{ToolCall, tools::ToolExecutionError};
use mf_runtime::NodeEnum;
use snafu::ensure;

#[derive(Clone, Debug, PartialEq, Eq, NodeEnum)]
#[value(tag = "kind")]
pub enum ToolActivity {
    #[value(rename = "call")]
    Call {
        model_call_id: String,
        part: u64,
        arguments: String,
        call: Option<ToolCall>,
    },
    #[value(rename = "running")]
    Running { tool_call_id: String, name: String },
    #[value(rename = "result")]
    Result {
        tool_call_id: String,
        failed: bool,
        content: String,
    },
}

impl ToolActivity {
    pub fn stream_name(&self) -> &'static str {
        match self {
            Self::Call { .. } => "tool_call",
            Self::Running { .. } => "tool_started",
            Self::Result { .. } => "tool_finished",
        }
    }

    pub fn validate(&self) -> Result<(), ToolExecutionError> {
        let valid = match self {
            Self::Call {
                model_call_id,
                call,
                ..
            } => {
                !model_call_id.trim().is_empty()
                    && call.as_ref().is_none_or(|call| {
                        !call.id.trim().is_empty() && !call.name.trim().is_empty()
                    })
            }
            Self::Running { tool_call_id, .. } => !tool_call_id.trim().is_empty(),
            Self::Result { tool_call_id, .. } => !tool_call_id.trim().is_empty(),
        };
        ensure!(valid, crate::tools::InvalidActivitySnafu);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn activity_codecs_are_strict_and_validate_correlations_without_interpreting_payloads() {
        for event in [
            json!({"kind":"call","model_call_id":"model","part":1,"arguments":"{"}),
            json!({"kind":"call","model_call_id":"model","part":1,"arguments":"","call":{"id":"id","name":"lookup","arguments":{"query":"x"}}}),
            json!({"kind":"running","tool_call_id":"id","name":"lookup"}),
            json!({"kind":"result","tool_call_id":"id","failed":true,"content":"**literal**"}),
        ] {
            let decoded: ToolActivity = serde_json::from_value(event.clone()).unwrap();
            decoded.validate().unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), event);
        }
        for event in [
            json!({"kind":"call","model_call_id":" ","part":0,"arguments":"x"}),
            json!({"kind":"call","model_call_id":"model","part":0,"arguments":"","call":{"id":"","name":"lookup","arguments":{}}}),
            json!({"kind":"call","model_call_id":"model","part":0,"arguments":"","call":{"id":"id","name":" ","arguments":{}}}),
            json!({"kind":"running","tool_call_id":"","name":"lookup"}),
            json!({"kind":"result","tool_call_id":"","failed":false,"content":""}),
        ] {
            assert!(
                serde_json::from_value::<ToolActivity>(event)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        for event in [
            json!({"kind":"unknown"}),
            json!({"kind":"result","tool_call_id":"id","failed":false,"content":"","extra":1}),
            json!({"kind":"running","tool_call_id":"id"}),
        ] {
            assert!(serde_json::from_value::<ToolActivity>(event).is_err());
        }
    }
}
