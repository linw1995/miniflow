use crate::{ChatError, Invocation, TextDelta, error::*};
use mf_runtime::{NodeValue, StreamCancellation, StreamDelivery};
use snafu::{ResultExt, ensure};
use std::sync::mpsc;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, NodeValue)]
pub struct PreviewEvent {
    pub invocation: Invocation,
    pub delta: TextDelta,
}

impl PreviewEvent {
    pub fn validate(&self) -> Result<(), ChatError> {
        ensure!(
            !self.delta.text.is_empty()
                && !self.delta.model_call_id.trim().is_empty()
                && !self.invocation.session_id.trim().is_empty()
                && !self.invocation.turn_id.trim().is_empty()
                && !self.invocation.call_id.trim().is_empty(),
            InvalidSnafu {
                field: "conversation preview",
                reason: "requires text, invocation, and model call identities"
            }
        );
        Ok(())
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
