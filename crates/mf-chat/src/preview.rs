use crate::{ChatError, Invocation, TextDelta, ToolCallDelta, error::*};
use mf_runtime::{NodeEnum, NodeValue, StreamCancellation, StreamDelivery};
use snafu::ensure;
use std::sync::{Arc, mpsc};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, NodeEnum)]
#[serde(tag = "kind")]
#[value(tag = "kind")]
pub enum PreviewDelta {
    Text { delta: TextDelta },
    ToolCall { delta: ToolCallDelta },
}

impl From<TextDelta> for PreviewDelta {
    fn from(delta: TextDelta) -> Self {
        Self::Text { delta }
    }
}

impl From<ToolCallDelta> for PreviewDelta {
    fn from(delta: ToolCallDelta) -> Self {
        Self::ToolCall { delta }
    }
}

impl PreviewDelta {
    pub fn validate(&self) -> Result<(), ChatError> {
        match self {
            Self::Text { delta } => ensure!(
                !delta.text.is_empty() && !delta.model_call_id.trim().is_empty(),
                InvalidSnafu {
                    field: "answer stream",
                    reason: "requires nonempty text and a model call identity"
                }
            ),
            Self::ToolCall { delta } => ensure!(
                !delta.model_call_id.trim().is_empty()
                    && (!delta.arguments.is_empty()
                        || [&delta.id, &delta.name]
                            .into_iter()
                            .flatten()
                            .any(|hint| !hint.trim().is_empty())),
                InvalidSnafu {
                    field: "tool_call_delta stream",
                    reason: "requires a model call identity and an argument fragment or wire hint"
                }
            ),
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, NodeValue)]
pub struct PreviewEvent {
    pub invocation: Invocation,
    pub delta: PreviewDelta,
}

impl PreviewEvent {
    pub fn validate(&self) -> Result<(), ChatError> {
        ensure!(
            !self.invocation.session_id.trim().is_empty()
                && !self.invocation.turn_id.trim().is_empty()
                && !self.invocation.call_id.trim().is_empty(),
            InvalidSnafu {
                field: "conversation preview",
                reason: "requires nonblank invocation identities"
            }
        );
        self.delta.validate()
    }
}

type Consume = dyn Fn(PreviewEvent, &StreamCancellation) -> Result<(), ChatError> + Send + Sync;

pub struct PreviewConsumer {
    consume: Box<Consume>,
    pub(crate) seal_drafts: Option<Arc<dyn Fn() + Send + Sync>>,
}

pub struct PreviewReceiver(mpsc::Receiver<PreviewEvent>);

const PREVIEW_QUEUE_CAPACITY: usize = 64;

impl PreviewConsumer {
    /// Runs a callback while draining the Flow, independently of rendering.
    /// Keep it nonblocking; an error from the callback fails native delivery.
    pub fn new(
        consume: impl Fn(PreviewEvent, &StreamCancellation) -> Result<(), ChatError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            consume: Box::new(consume),
            seal_drafts: None,
        }
    }

    /// Pin any display read view immediately before storage retires its drafts. This
    /// callback must not wait for subscription receipts, rendering, or provider I/O.
    pub fn with_draft_seal(mut self, seal: impl Fn() + Send + Sync + 'static) -> Self {
        self.seal_drafts = Some(Arc::new(seal));
        self
    }

    pub fn send(
        &self,
        event: PreviewEvent,
        receipt: StreamDelivery,
        cancellation: &StreamCancellation,
    ) -> Result<(), ChatError> {
        check_cancellation(cancellation)?;
        (self.consume)(event, cancellation)?;
        receipt.acknowledge()?;
        check_cancellation(cancellation)
    }

    /// Offers previews to a bounded subscription without waiting for its reader.
    /// New previews are discarded when the queue is full or its receiver is dropped.
    pub fn channel() -> (Self, PreviewReceiver) {
        let (sender, receiver) = mpsc::sync_channel(PREVIEW_QUEUE_CAPACITY);
        let consumer = Self::new(move |event, _| {
            let _ = sender.try_send(event);
            Ok(())
        });
        (consumer, PreviewReceiver(receiver))
    }
}

fn check_cancellation(cancellation: &StreamCancellation) -> Result<(), ChatError> {
    if let Some(error) = cancellation.failure() {
        return Err(error.into());
    }
    Ok(())
}

impl PreviewReceiver {
    pub fn try_recv(&self) -> Result<PreviewEvent, mpsc::TryRecvError> {
        self.0.try_recv()
    }

    pub fn recv(&self) -> Result<PreviewEvent, mpsc::RecvError> {
        self.0.recv()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_native_and_serde_codecs_preserve_text_and_tool_event_kinds() {
        let invocation = Invocation::new("session");
        for delta in [
            TextDelta {
                text: "text".into(),
                model_call_id: "model".into(),
            }
            .into(),
            ToolCallDelta {
                model_call_id: "model".into(),
                index: 0,
                arguments: "{\"query\":".into(),
                id: Some("call".into()),
                name: None,
            }
            .into(),
        ] {
            let event = PreviewEvent {
                invocation: invocation.clone(),
                delta,
            };
            event.validate().unwrap();
            assert_eq!(
                PreviewEvent::from_values(event.clone().into_values().unwrap()).unwrap(),
                event
            );
            assert_eq!(
                serde_json::from_value::<PreviewEvent>(serde_json::to_value(&event).unwrap())
                    .unwrap(),
                event
            );
        }
        let mut event = PreviewEvent {
            invocation,
            delta: TextDelta {
                text: "text".into(),
                model_call_id: "model".into(),
            }
            .into(),
        };
        event.invocation.call_id.clear();
        assert!(event.validate().is_err());
    }
}
