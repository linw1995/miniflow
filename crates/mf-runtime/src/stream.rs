use crate::{ExecutionContext, Inputs, NodeExecutionError, Outputs, TypedNodeResult};
use serde::{Deserialize, Serialize};
use snafu::ResultExt;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamMode {
    Stream,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamExecution {
    pub mode: StreamMode,
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

#[derive(Debug)]
pub enum NodeEvent<I = Inputs> {
    Input(I),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlushReason {
    SizeExceed,
    TimeoutExceed,
    UpstreamClosed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchInfo {
    pub item_count: usize,
    pub reason: FlushReason,
}

#[derive(Clone, Debug)]
pub struct EventEmission<O = Outputs> {
    pub result: TypedNodeResult<O>,
    pub batch: Option<BatchInfo>,
}

impl<O> From<TypedNodeResult<O>> for EventEmission<O> {
    fn from(result: TypedNodeResult<O>) -> Self {
        Self {
            result,
            batch: None,
        }
    }
}

#[derive(Debug)]
pub struct EventEffects<O = Outputs> {
    pub emissions: Vec<EventEmission<O>>,
    pub timer: TimerUpdate,
}

impl<O> Default for EventEffects<O> {
    fn default() -> Self {
        Self {
            emissions: Vec::new(),
            timer: TimerUpdate::default(),
        }
    }
}

/// State is owned by one workflow instance and invoked serially by its coordinator.
pub trait EventNode: Send {
    fn on_event(
        &mut self,
        event: NodeEvent,
        context: &EventContext<'_>,
    ) -> Result<EventEffects, NodeExecutionError>;

    fn buffered_items(&self) -> Option<usize> {
        None
    }
}

/// An instance-owned producer invoked serially on a dedicated worker.
pub trait StreamNode: Send {
    /// Returning ends this input invocation; successful sends may still be draining.
    fn execute(
        &mut self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut crate::Emitter<'_>,
    ) -> Result<(), NodeExecutionError>;
}

/// Serial event execution with runtime-owned input decoding and emission encoding.
pub trait TypedEventNode: Send {
    type Input: crate::NodeInputs;
    type Output: crate::NodeOutputs;

    fn on_event(
        &mut self,
        event: NodeEvent<Self::Input>,
        context: &EventContext<'_>,
    ) -> Result<EventEffects<Self::Output>, NodeExecutionError>;

    fn buffered_items(&self) -> Option<usize> {
        None
    }
}

/// An incremental producer whose callback accepts only its declared output contract.
pub trait TypedStreamNode: Send {
    type Input: crate::NodeInputs;
    type Output: crate::NodeOutputs;

    fn execute(
        &mut self,
        input: Self::Input,
        context: &mut ExecutionContext,
        emit: &mut dyn FnMut(TypedNodeResult<Self::Output>) -> Result<(), NodeExecutionError>,
    ) -> Result<(), NodeExecutionError>;
}

pub fn execute_typed_event<N: TypedEventNode + ?Sized>(
    state: &mut N,
    event: NodeEvent,
    context: &EventContext<'_>,
) -> Result<EventEffects, NodeExecutionError> {
    let event = match event {
        NodeEvent::Input(inputs) => NodeEvent::Input(
            <N::Input as crate::NodeInputs>::from_inputs(inputs)
                .context(crate::node::InputDecodeSnafu)?,
        ),
        NodeEvent::Timer => NodeEvent::Timer,
        NodeEvent::UpstreamClosed => NodeEvent::UpstreamClosed,
    };
    let effects = state.on_event(event, context)?;
    let emissions = effects
        .emissions
        .into_iter()
        .map(|emission| {
            Ok(EventEmission {
                result: crate::encode_typed_result(emission.result)?,
                batch: emission.batch,
            })
        })
        .collect::<Result<_, NodeExecutionError>>()?;
    Ok(EventEffects {
        emissions,
        timer: effects.timer,
    })
}

pub(super) struct TypedEventAdapter<N>(pub N);

impl<N: TypedEventNode> EventNode for TypedEventAdapter<N> {
    fn on_event(
        &mut self,
        event: NodeEvent,
        context: &EventContext<'_>,
    ) -> Result<EventEffects, NodeExecutionError> {
        execute_typed_event(&mut self.0, event, context)
    }

    fn buffered_items(&self) -> Option<usize> {
        self.0.buffered_items()
    }
}

pub(super) struct TypedStreamAdapter<N>(pub N);

impl<N: TypedStreamNode> StreamNode for TypedStreamAdapter<N> {
    fn execute(
        &mut self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut crate::Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        let input = <N::Input as crate::NodeInputs>::from_inputs(inputs)
            .context(crate::node::InputDecodeSnafu)?;
        self.0.execute(input, context, &mut |result| {
            emitter.send(crate::encode_typed_result(result)?)
        })
    }
}
