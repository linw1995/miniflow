//! Turn-scoped host implementation of the Agent's zero-argument steering callback.

use crate::{
    ChatError, ChatMessage, Invocation, PendingMessageId, error::*, input_queue::PendingMessages,
};
use mf_runtime::{
    OutputValue, ValueRef,
    callback::{CallbackReply, HostCallback},
};
use snafu::{ResultExt, ensure};
use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    sync::{Arc, Mutex},
};

#[cfg(test)]
use crate::input_queue::{MAX_PENDING_BYTES, MAX_PENDING_MESSAGES};
#[cfg(test)]
mod tests;

pub const CALLBACK_NAME: &str = "steering";

/// Clones share only this invocation's FIFO. Closing admission never submits a new turn.
#[derive(Clone)]
pub struct SteeringInbox {
    invocation: Invocation,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    closed: bool,
    pending: PendingMessages,
}

impl SteeringInbox {
    pub fn new(invocation: Invocation) -> Self {
        Self {
            invocation,
            state: Arc::default(),
        }
    }
    pub fn invocation(&self) -> &Invocation {
        &self.invocation
    }
    pub fn submit(&self, text: String) -> Result<PendingMessageId, ChatError> {
        let mut state = self.state.lock().unwrap();
        ensure!(!state.closed, SteeringClosedSnafu);
        state.pending.push(text)
    }
    /// Withdrawal and claiming a batch linearize under the same lock.
    pub fn withdraw(&self, id: PendingMessageId) -> Option<String> {
        self.state.lock().unwrap().pending.withdraw(id)
    }
    pub fn withdraw_last(&self) -> Option<String> {
        self.state.lock().unwrap().pending.pop_back()
    }
    pub fn close(&self) {
        self.state.lock().unwrap().closed = true;
    }
    /// Retrieve undelivered input for editing, never for automatic replay.
    pub fn take_unsent(&self) -> Vec<String> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            state.pending.drain().collect()
        } else {
            Vec::new()
        }
    }
    fn take(&self) -> Result<VecDeque<(PendingMessageId, String)>, ChatError> {
        let mut state = self.state.lock().unwrap();
        ensure!(!state.closed, SteeringClosedSnafu);
        Ok(std::mem::take(&mut state.pending.0))
    }
}

pub struct DeliveryLog {
    inbox: SteeringInbox,
    in_flight: std::sync::atomic::AtomicBool,
    messages: Mutex<Vec<String>>,
}

impl DeliveryLog {
    pub fn new(inbox: SteeringInbox) -> Arc<Self> {
        Arc::new(Self {
            inbox,
            in_flight: Default::default(),
            messages: Mutex::default(),
        })
    }

    pub fn callback(self: &Arc<Self>) -> HostCallback {
        let log = self.clone();
        HostCallback::new(move |args, _| {
            let run = || -> Result<CallbackReply, ChatError> {
                ensure!(
                    args.0.is_empty(),
                    InvalidSnafu {
                        field: "steering callback",
                        reason: "does not accept arguments",
                    }
                );
                log.claim()
            };
            run().boxed().context(mf_runtime::callback::HandlerSnafu)
        })
    }

    fn claim(self: &Arc<Self>) -> Result<CallbackReply, ChatError> {
        use std::sync::atomic::Ordering;
        ensure!(
            !self.in_flight.swap(true, Ordering::AcqRel),
            InvalidSnafu {
                field: "steering callback",
                reason: "a previous delivery is still in flight"
            }
        );
        let mut batch = match self.inbox.take() {
            Ok(batch) => batch,
            Err(error) => {
                self.in_flight.store(false, Ordering::Release);
                return Err(error);
            }
        };
        let result = batch
            .iter()
            .map(|(_, text)| {
                ChatMessage::User {
                    content: text.clone(),
                    metadata: None,
                }
                .encode()
                .map(ValueRef::into_secret)
            })
            .collect::<Result<Vec<_>, _>>()
            .context(SteeringValueSnafu)
            .map(|values| {
                CallbackReply::from(
                    [("messages".into(), ValueRef::array(values))]
                        .into_iter()
                        .collect::<mf_runtime::FlowOutputs>(),
                )
            });
        match result {
            Ok(reply) if batch.is_empty() => {
                self.in_flight.store(false, Ordering::Release);
                Ok(reply)
            }
            Ok(reply) => {
                let log = self.clone();
                Ok(reply.on_delivery(move |delivered| {
                    if delivered {
                        log.messages
                            .lock()
                            .unwrap()
                            .extend(batch.drain(..).map(|(_, text)| text));
                    } else {
                        log.restore(batch);
                    }
                    log.in_flight.store(false, Ordering::Release);
                }))
            }
            Err(error) => {
                self.restore(batch);
                self.in_flight.store(false, Ordering::Release);
                Err(error)
            }
        }
    }

    fn restore(&self, mut batch: VecDeque<(PendingMessageId, String)>) {
        let mut state = self.inbox.state.lock().unwrap();
        state.closed = true;
        batch.append(&mut state.pending.0);
        state.pending.0 = batch;
    }
}

/// Receipts survive a save-only retry in the owning host, never a restart.
#[derive(Default)]
pub struct Deliveries {
    calls: Mutex<HashMap<String, Arc<DeliveryLog>>>,
}

impl Deliveries {
    pub fn with_active<T>(
        &self,
        invocation: &Invocation,
        log: Option<&Arc<DeliveryLog>>,
        run: impl FnOnce() -> Result<T, ChatError>,
    ) -> Result<T, ChatError> {
        let Some(log) = log else { return run() };
        ensure!(
            log.inbox.invocation() == invocation,
            ConflictSnafu {
                id: &invocation.call_id
            }
        );
        match self.calls.lock().unwrap().entry(invocation.call_id.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(log.clone());
            }
            Entry::Occupied(_) => {
                return ConflictSnafu {
                    id: &invocation.call_id,
                }
                .fail();
            }
        }
        let result = run();
        log.inbox.close();
        result
    }

    pub fn validate(&self, call_id: &str, display: &[ChatMessage]) -> Result<(), ChatError> {
        let receipts = self.calls.lock().unwrap().get(call_id).cloned();
        let delivered = receipts.as_ref().map(|log| log.messages.lock().unwrap());
        let expected = delivered.as_deref().map_or(&[][..], Vec::as_slice);
        let users = display.iter().filter_map(|message| match message {
            ChatMessage::User { content, .. } => Some(content),
            _ => None,
        });
        ensure!(
            users.eq(expected.iter()),
            InvalidSnafu {
                field: "display steering",
                reason: "must exactly match host-delivered user input in order"
            }
        );
        Ok(())
    }
    pub fn forget(&self, call_id: &str) {
        if let Some(log) = self.calls.lock().unwrap().remove(call_id) {
            log.inbox.close();
        }
    }
}
