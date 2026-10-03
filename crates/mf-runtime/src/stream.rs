use crate::stream_plan::InvalidPlanSnafu;
use crate::{
    ExecutionContext, Inputs, NodeExecutionError, NodeResult, StreamBuildError, ValueType,
};
use serde::{Deserialize, Serialize};
use snafu::ensure;
use std::time::Duration;

pub const STREAM_INPUT_ID: &str = "%input";

pub fn stream_input_node(value_type: ValueType) -> crate::FlowNode {
    crate::FlowNode {
        definition_id: STREAM_INPUT_ID.into(),
        node: None,
        metadata: crate::NodePorts {
            inputs: Vec::new(),
            outputs: vec![crate::PortSpec::new("item", value_type, true)],
        }
        .into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamMode {
    Stream,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamExecution {
    pub mode: StreamMode,
    pub input_type: ValueType,
    #[serde(default)]
    pub limits: StreamLimits,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StreamLimits {
    pub max_pending_messages: usize,
    pub workers: usize,
}

impl Default for StreamLimits {
    fn default() -> Self {
        Self {
            max_pending_messages: 64,
            workers: 4,
        }
    }
}

impl StreamLimits {
    pub fn validate(&self) -> Result<(), StreamBuildError> {
        ensure!(
            ![self.max_pending_messages, self.workers].contains(&0),
            InvalidPlanSnafu {
                message: "stream limits must be positive",
            }
        );
        Ok(())
    }
}

#[derive(Debug)]
pub enum NodeEvent {
    Input(Inputs),
    Timer,
    UpstreamClosed,
}

pub struct EventContext<'a> {
    pub now: Duration,
    /// Read current-frame references; publish through returned emissions.
    pub input: Option<&'a ExecutionContext>,
}

/// Updates one coordinator-owned deadline; timer callbacks are delivered serially, not queued.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimerUpdate {
    #[default]
    Keep,
    Cancel,
    Set(Duration),
}

#[derive(Clone, Debug)]
pub struct EventEmission {
    pub result: NodeResult,
}

impl From<NodeResult> for EventEmission {
    fn from(result: NodeResult) -> Self {
        Self { result }
    }
}

#[derive(Debug, Default)]
pub struct EventEffects {
    pub emissions: Vec<EventEmission>,
    pub timer: TimerUpdate,
}

/// State is owned by one workflow instance and invoked serially by its coordinator.
pub trait EventNode: Send {
    fn on_event(
        &mut self,
        event: NodeEvent,
        context: &EventContext<'_>,
    ) -> Result<EventEffects, NodeExecutionError>;
}
