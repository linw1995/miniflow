//! In-memory input waiting for a new turn, never for the current runner.

use crate::{ChatError, Completion, Invocation, error::*};
use snafu::ensure;
use std::collections::VecDeque;
use uuid::Uuid;

pub const MAX_PENDING_BYTES: usize = 256 * 1024;
pub const MAX_PENDING_MESSAGES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingMessageId(Uuid);

pub struct PendingMessages<T = String>(pub VecDeque<(PendingMessageId, T)>);

impl<T> Default for PendingMessages<T> {
    fn default() -> Self {
        Self(VecDeque::new())
    }
}

impl<T: AsRef<str>> PendingMessages<T> {
    pub fn push(&mut self, message: T) -> Result<PendingMessageId, ChatError> {
        let text = message.as_ref();
        ensure!(
            !text.trim().is_empty(),
            InvalidSnafu {
                field: "pending message",
                reason: "must contain nonblank text"
            }
        );
        ensure!(
            self.0.len() < MAX_PENDING_MESSAGES
                && text
                    .len()
                    .saturating_add(self.0.iter().map(|(_, item)| item.as_ref().len()).sum())
                    <= MAX_PENDING_BYTES,
            InvalidSnafu {
                field: "pending messages",
                reason: "pending input exceeds its message or byte limit"
            }
        );
        let id = PendingMessageId(Uuid::new_v4());
        self.0.push_back((id, message));
        Ok(id)
    }

    pub fn withdraw(&mut self, id: PendingMessageId) -> Option<T> {
        let index = self.0.iter().position(|(pending, _)| *pending == id)?;
        self.0.remove(index).map(|(_, item)| item)
    }

    pub fn pop_back(&mut self) -> Option<T> {
        self.0.pop_back().map(|(_, item)| item)
    }

    pub fn drain(&mut self) -> impl Iterator<Item = T> {
        self.0.drain(..).map(|(_, item)| item)
    }
}

/// The host has claimed the prompt, but its new turn may still fail durable admission.
pub struct ClaimedTurn<T> {
    pub id: PendingMessageId,
    pub invocation: Invocation,
    pub message: T,
}

/// A session's FIFO during one uninterrupted series of turns. The owner serializes
/// admission, withdrawal and settlement. An entry can carry a caller's receipt with its text.
/// A save-only retry never drives this queue.
pub struct NextTurnQueue<T = String> {
    current: Invocation,
    pending: PendingMessages<T>,
    closed: bool,
}

impl<T: AsRef<str>> NextTurnQueue<T> {
    pub fn new(current: Invocation) -> Self {
        Self {
            current,
            pending: PendingMessages::default(),
            closed: false,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.current.session_id
    }

    pub fn enqueue(&mut self, message: T) -> Result<PendingMessageId, ChatError> {
        ensure!(!self.closed, NextTurnClosedSnafu);
        self.pending.push(message)
    }

    pub fn withdraw(&mut self, id: PendingMessageId) -> Option<T> {
        self.pending.withdraw(id)
    }

    pub fn withdraw_last(&mut self) -> Option<T> {
        self.pending.pop_back()
    }

    /// Claim at most one prompt, only against the exact successful predecessor. Closing an
    /// empty queue here makes an enqueue racing with the last commit either run or fail visibly.
    pub fn advance(&mut self, committed: Option<&Completion>) -> Option<ClaimedTurn<T>> {
        if !self.closed
            && committed.is_some_and(|saved| {
                saved.invocation == self.current && saved.result.status == "succeeded"
            })
            && let Some((id, message)) = self.pending.0.pop_front()
        {
            self.current = Invocation::new(self.session_id());
            return Some(ClaimedTurn {
                invocation: self.current.clone(),
                id,
                message,
            });
        }
        self.closed = true;
        None
    }

    /// Stop the series and return unclaimed prompts in submission order for editing, not replay.
    pub fn stop(&mut self) -> Vec<T> {
        self.closed = true;
        self.pending.drain().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConversationResult;
    use std::sync::{Barrier, Mutex};

    #[test]
    fn withdrawal_racing_a_commit_never_retracts_a_claimed_prompt() {
        let invocation = Invocation::new("session");
        let queue = Mutex::new(NextTurnQueue::new(invocation.clone()));
        let id = queue.lock().unwrap().enqueue("one".to_owned()).unwrap();
        let ready = Barrier::new(2);
        let saved = Completion {
            invocation,
            result: ConversationResult::assistant("done"),
        };
        std::thread::scope(|scope| {
            let claim = scope.spawn(|| {
                ready.wait();
                queue.lock().unwrap().advance(Some(&saved))
            });
            ready.wait();
            let withdrawn = queue.lock().unwrap().withdraw(id);
            let claimed = claim.join().unwrap().map(|turn| turn.message);
            assert_ne!(claimed.is_some(), withdrawn.is_some());
            assert_eq!(claimed.or(withdrawn).as_deref(), Some("one"));
        });
    }

    #[test]
    fn a_full_steering_inbox_does_not_consume_next_turn_capacity_or_ids() {
        let current = Invocation::new("s");
        let inbox = crate::SteeringInbox::new(current.clone());
        let mut queue = NextTurnQueue::new(current);
        let full = "x".repeat(MAX_PENDING_BYTES);
        let steer = inbox.submit(full.clone()).unwrap();
        let later = queue.enqueue(full.clone()).unwrap();
        assert!(inbox.submit("more".into()).is_err());
        assert!(queue.enqueue("more".into()).is_err());
        assert!(inbox.withdraw(later).is_none());
        assert!(queue.withdraw(steer).is_none());
        assert_eq!(queue.withdraw_last().as_ref(), Some(&full));
        assert_eq!(inbox.withdraw(steer), Some(full));
    }

    #[test]
    fn replayed_success_does_not_release_a_later_turns_queue() {
        let saved = Completion {
            invocation: Invocation::new("s"),
            result: ConversationResult::assistant("first reply"),
        };
        let mut queue = NextTurnQueue::new(saved.invocation.clone());
        queue.enqueue("two").unwrap();
        queue.enqueue("three").unwrap();
        assert_eq!(queue.advance(Some(&saved)).unwrap().message, "two");
        assert!(queue.advance(Some(&saved)).is_none());
        assert!(queue.enqueue("late").is_err());
        assert_eq!(queue.stop(), ["three"]);
        assert!(queue.stop().is_empty());
    }
}
