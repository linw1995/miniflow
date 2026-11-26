//! Turn-scoped host implementation of the Agent's ordinary reply-boundary call.

use crate::{
    ChatError, ChatMessage, Invocation, PendingMessageId, error::*, input_queue::PendingMessages,
};
use mf_runtime::{
    OutputValue, ValueRef,
    callback::{CallbackReply, HostCallback},
};
use snafu::{OptionExt, ResultExt, ensure};
use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    sync::{Arc, Mutex},
};

#[cfg(test)]
use crate::input_queue::{MAX_PENDING_BYTES, MAX_PENDING_MESSAGES};
#[cfg(test)]
mod tests;

pub const ON_REPLY: &str = "on_reply";

#[derive(Clone, Copy)]
enum PullMode {
    Open,
    Last,
    Close,
}

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
    /// Withdrawal and claiming a boundary batch linearize under the same lock.
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
    fn take_at(&self, mode: PullMode) -> Result<VecDeque<(PendingMessageId, String)>, ChatError> {
        let mut state = self.state.lock().unwrap();
        ensure!(!state.closed, SteeringClosedSnafu);
        match mode {
            PullMode::Open => {}
            PullMode::Last if !state.pending.0.is_empty() => {}
            _ => {
                state.closed = true;
                return Ok(VecDeque::new());
            }
        }
        Ok(std::mem::take(&mut state.pending.0))
    }
}

pub struct DeliveryLog {
    inbox: SteeringInbox,
    in_flight: std::sync::atomic::AtomicBool,
    messages: Mutex<Vec<(i64, String)>>,
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
                use mf_runtime::InputValue;
                let invalid = || InvalidSnafu {
                    field: "reply boundary",
                    reason: "requires reply.position and reply.outcome",
                };
                ensure!(args.0.len() == 2, invalid());
                let position = args
                    .0
                    .get("reply.position")
                    .and_then(ValueRef::as_i64)
                    .with_context(invalid)?;
                let outcome = args.0.get("reply.outcome").cloned().with_context(invalid)?;
                let mode = match mfn_llm::CallOutcome::decode(outcome).context(ReplyValueSnafu)? {
                    mfn_llm::CallOutcome::ToolCalls => PullMode::Open,
                    mfn_llm::CallOutcome::Completed => PullMode::Last,
                    _ => PullMode::Close,
                };
                log.claim(mode, position)
            };
            run().boxed().context(mf_runtime::callback::HandlerSnafu)
        })
    }

    fn claim(self: &Arc<Self>, mode: PullMode, position: i64) -> Result<CallbackReply, ChatError> {
        use std::sync::atomic::Ordering;
        ensure!(
            !self.in_flight.swap(true, Ordering::AcqRel),
            InvalidSnafu {
                field: "reply boundary",
                reason: "a previous delivery is still in flight"
            }
        );
        let mut batch = match self.inbox.take_at(mode) {
            Ok(batch) => batch,
            Err(error) => {
                self.in_flight.store(false, Ordering::Release);
                return Err(error);
            }
        };
        let result = (|| {
            let receipts = self.messages.lock().unwrap();
            ensure!(
                position >= 0
                    && (batch.is_empty()
                        || position > 0
                            && receipts.last().is_none_or(|(last, _)| *last < position)),
                InvalidSnafu {
                    field: "steering position",
                    reason: "must advance past the previous delivery at a completed reply"
                }
            );
            if !batch.is_empty() {
                position
                    .checked_add((batch.len() - 1) as i64)
                    .context(ArithmeticSnafu {
                        operation: "position steering messages",
                    })?;
            }
            let values = batch
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
                .context(ReplyValueSnafu)?;
            Ok(CallbackReply::from(
                [("messages".into(), ValueRef::array(values))]
                    .into_iter()
                    .collect::<mf_runtime::FlowOutputs>(),
            ))
        })();
        match result {
            Ok(reply) if batch.is_empty() => {
                self.in_flight.store(false, Ordering::Release);
                Ok(reply)
            }
            Ok(reply) => {
                let log = self.clone();
                Ok(reply.on_delivery(move |delivered| {
                    if delivered {
                        log.messages.lock().unwrap().extend(
                            batch
                                .drain(..)
                                .enumerate()
                                .map(|(i, (_, text))| (position + i as i64, text)),
                        );
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
        let users = display
            .iter()
            .enumerate()
            .filter_map(|(index, message)| match message {
                ChatMessage::User { content, .. } => Some((index as i64, content)),
                _ => None,
            });
        ensure!(
            users.eq(expected.iter().map(|(index, text)| (*index, text))),
            InvalidSnafu {
                field: "display steering",
                reason: "must exactly match host-delivered user input at its reply boundaries"
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
