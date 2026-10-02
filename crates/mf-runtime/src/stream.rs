use crate::{ExecutionContext, Inputs, NodeExecutionError, NodeResult};
use std::time::Duration;

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
