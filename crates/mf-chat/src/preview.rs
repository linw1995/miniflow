use crate::{ChatError, Invocation, TextDelta, error::*};
use mf_runtime::{NodeValue, StreamCancellation};
use snafu::{ResultExt, ensure};
use std::{
    error::Error,
    sync::{Arc, mpsc},
    time::Duration,
};

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

type Consume = dyn Fn(PreviewEvent, &StreamCancellation) -> Result<(), ChatError> + Send + Sync;

#[derive(Clone)]
pub struct PreviewConsumer(Arc<Consume>);

struct Delivery {
    event: PreviewEvent,
    received: mpsc::SyncSender<()>,
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
        Self(Arc::new(consume))
    }

    pub fn send(
        &self,
        event: PreviewEvent,
        cancellation: &StreamCancellation,
    ) -> Result<(), ChatError> {
        check_cancellation(cancellation)?;
        (self.0)(event, cancellation)?;
        check_cancellation(cancellation)
    }

    pub fn channel() -> (Self, PreviewReceiver) {
        let (sender, receiver) = mpsc::sync_channel(1);
        let consumer = Self::new(move |event, cancellation| {
            let (received, receipt) = mpsc::sync_channel(1);
            let mut delivery = Delivery { event, received };
            loop {
                check_cancellation(cancellation)?;
                match sender.try_send(delivery) {
                    Ok(()) => break,
                    Err(mpsc::TrySendError::Full(pending)) => delivery = pending,
                    Err(error) => {
                        return Err(Box::<dyn Error + Send + Sync>::from(error))
                            .context(PreviewDeliverySnafu);
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            // Keep the native Stream delivery unacknowledged until the caller
            // takes the preview, so settlement cannot overtake unread data.
            loop {
                check_cancellation(cancellation)?;
                match receipt.recv_timeout(Duration::from_millis(10)) {
                    Ok(()) => return Ok(()),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(error) => {
                        return Err(Box::<dyn Error + Send + Sync>::from(error))
                            .context(PreviewDeliverySnafu);
                    }
                }
            }
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
        self.0.try_recv().map(consume)
    }

    pub fn recv(&self) -> Result<PreviewEvent, mpsc::RecvError> {
        self.0.recv().map(consume)
    }
}

fn consume(delivery: Delivery) -> PreviewEvent {
    // A cancelled producer may have stopped waiting for this receipt.
    let _ = delivery.received.try_send(());
    delivery.event
}
