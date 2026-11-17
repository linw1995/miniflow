use crate::{ChatError, Invocation, TextDelta, error::*};
use mf_runtime::{NodeEnum, NodeValue, StreamCancellation, StreamDelivery};
use snafu::{ResultExt, ensure};
use std::sync::mpsc;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, NodeEnum)]
#[serde(tag = "kind")]
#[value(tag = "kind")]
pub enum PreviewDelta {
    Text { delta: TextDelta },
    Tool { activity: crate::ToolActivity },
}

impl From<TextDelta> for PreviewDelta {
    fn from(delta: TextDelta) -> Self {
        Self::Text { delta }
    }
}

impl From<crate::ToolActivity> for PreviewDelta {
    fn from(activity: crate::ToolActivity) -> Self {
        Self::Tool { activity }
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
            Self::Tool { activity } => activity.validate().boxed().context(PreviewDeliverySnafu)?,
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
                reason: "requires text, invocation, and model call identities"
            }
        );
        self.delta.validate()
    }
}

type Consume = dyn Fn(PreviewEvent, StreamDelivery, &StreamCancellation) -> Result<(), ChatError>
    + Send
    + Sync;

pub struct PreviewConsumer(Box<Consume>);

struct Delivery {
    event: PreviewEvent,
    receipt: StreamDelivery,
}

pub struct PreviewReceiver(mpsc::Receiver<Delivery>);

impl std::fmt::Debug for Delivery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Delivery").finish_non_exhaustive()
    }
}

impl PreviewConsumer {
    pub fn new(
        consume: impl Fn(PreviewEvent, &StreamCancellation) -> Result<(), ChatError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Box::new(move |event, receipt, cancellation| {
            consume(event, cancellation)?;
            receipt.acknowledge()?;
            Ok(())
        }))
    }

    pub fn send(
        &self,
        event: PreviewEvent,
        receipt: StreamDelivery,
        cancellation: &StreamCancellation,
    ) -> Result<(), ChatError> {
        check_cancellation(cancellation)?;
        (self.0)(event, receipt, cancellation)?;
        check_cancellation(cancellation)
    }

    pub fn channel() -> (Self, PreviewReceiver) {
        // Native receipts permit only one outstanding preview per invocation.
        let (sender, receiver) = mpsc::channel();
        let consumer = Self(Box::new(move |event, receipt, _| {
            sender
                .send(Delivery { event, receipt })
                .boxed()
                .context(PreviewDeliverySnafu)
        }));
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
        self.0.try_recv().map(consume)
    }

    pub fn recv(&self) -> Result<PreviewEvent, mpsc::RecvError> {
        self.0.recv().map(consume)
    }
}

fn consume(delivery: Delivery) -> PreviewEvent {
    // Acknowledgement releases the native message domain after the caller takes its preview.
    let _ = delivery.receipt.acknowledge();
    delivery.event
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
            crate::ToolActivity::Running {
                tool_call_id: "tool".into(),
                name: "lookup".into(),
            }
            .into(),
            crate::ToolActivity::Result {
                tool_call_id: "tool".into(),
                failed: true,
                content: "**literal**".into(),
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
        let invalid = PreviewDelta::Tool {
            activity: crate::ToolActivity::Running {
                tool_call_id: String::new(),
                name: "lookup".into(),
            },
        };
        let error = invalid.validate().unwrap_err();
        assert!(std::error::Error::source(&error).is_some());
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
