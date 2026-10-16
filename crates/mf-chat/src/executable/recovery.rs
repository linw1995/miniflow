//! Bounded model replies retained by the owning application for save-only retry.

use super::{ChatContract, InvocationState, TurnExit};
use crate::{
    CallPurpose, ChatDatabase, ChatError, Completion, Invocation, UncommittedCompletion, error::*,
    turn,
};
use mf_runtime::NodeValue;
use snafu::{IntoError, ResultExt, ensure};

pub const MAX_RECOVERY_BYTES: usize = 16 * 1024 * 1024;

impl ChatContract {
    pub fn decode_completion(
        &self,
        bytes: &[u8],
        invocation: &Invocation,
    ) -> Result<Completion, ChatError> {
        ensure!(
            bytes.len() <= MAX_RECOVERY_BYTES,
            RecoveryTooLargeSnafu {
                limit: MAX_RECOVERY_BYTES
            }
        );
        let values: mf_runtime::NodeValues =
            mf_runtime::decode_unique_json(bytes).context(JsonSnafu)?;
        let response = mfn_llm::LlmOutputs::from_values(values)?;
        let completion = Completion {
            invocation: invocation.clone(),
            reply: response.into(),
        };
        turn::validate_reply(&completion.reply)?;
        Ok(completion)
    }
}

#[derive(Clone, Debug)]
pub enum CompletionState {
    NotBegun,
    Committed(Completion),
    Uncommitted(UncommittedCompletion),
    Finished(crate::Turn),
}

pub struct ConversationExit<'a> {
    pub contract: &'a ChatContract,
    pub invocation: &'a Invocation,
    pub stdout: Option<&'a [u8]>,
    pub outcome: TurnExit,
    pub summary: &'a str,
}

pub struct ReconciledCompletion {
    pub state: CompletionState,
    pub diagnostics: Vec<ChatError>,
}

impl ChatDatabase {
    pub fn reconcile_completion(
        &self,
        exit: ConversationExit<'_>,
    ) -> Result<ReconciledCompletion, ChatError> {
        let stored = self.inspect_invocation(exit.invocation)?;
        let mut diagnostics = Vec::new();
        let delivered = exit
            .stdout
            .map(|bytes| exit.contract.decode_completion(bytes, exit.invocation));
        if let InvocationState::Committed(saved) = stored {
            match delivered {
                Some(Ok(completion)) if completion != saved => diagnostics.push(
                    InvalidSnafu {
                        field: "model reply",
                        reason: "disagrees with the committed reply",
                    }
                    .build(),
                ),
                Some(Err(source)) => diagnostics.push(source),
                _ => {}
            }
            return Ok(ReconciledCompletion {
                state: CompletionState::Committed(saved),
                diagnostics,
            });
        }
        match delivered {
            Some(Ok(completion)) if matches!(stored, InvocationState::Running) => {
                return Ok(ReconciledCompletion {
                    state: CompletionState::Uncommitted(UncommittedCompletion {
                        database_path: self.path().to_owned(),
                        completion,
                        purpose: CallPurpose::Conversation,
                    }),
                    diagnostics,
                });
            }
            Some(Err(source)) => diagnostics.push(source),
            Some(Ok(_)) => diagnostics.push(
                InvalidSnafu {
                    field: "model reply",
                    reason: "requires the parent's admitted invocation",
                }
                .build(),
            ),
            None => {}
        }
        let finalized =
            self.finalize_invocation(exit.invocation.clone(), exit.outcome, exit.summary)?;
        let state = match finalized {
            InvocationState::NotBegun => CompletionState::NotBegun,
            InvocationState::Committed(completion) => CompletionState::Committed(completion),
            InvocationState::Finished(turn) => CompletionState::Finished(turn),
            InvocationState::Running => {
                return InvalidSnafu {
                    field: "chat cleanup",
                    reason: "did not finalize the invocation",
                }
                .fail();
            }
        };
        Ok(ReconciledCompletion { state, diagnostics })
    }

    pub fn save_recovered(&self, pending: UncommittedCompletion) -> Result<Completion, ChatError> {
        ensure!(
            pending.purpose == CallPurpose::Conversation,
            InvalidSnafu {
                field: "purpose",
                reason: "use title recovery for a title response"
            }
        );
        ensure!(
            pending.database_path == self.path(),
            InvalidSnafu {
                field: "database_path",
                reason: "completion belongs to another host database"
            }
        );
        match self.complete_turn(
            pending.completion.invocation.clone(),
            pending.completion.reply.clone(),
        ) {
            Ok(completion) => Ok(completion),
            Err(source) => Err(UncommittedSnafu {
                pending: Box::new(pending),
            }
            .into_error(Box::new(source))),
        }
    }

    pub fn abandon_recovered(
        &self,
        pending: &UncommittedCompletion,
    ) -> Result<InvocationState, ChatError> {
        ensure!(
            pending.database_path == self.path() && pending.purpose == CallPurpose::Conversation,
            InvalidSnafu {
                field: "retained completion",
                reason: "belongs to another database or call purpose"
            }
        );
        self.finalize_invocation(
            pending.completion.invocation.clone(),
            TurnExit::Cancelled,
            "uncommitted reply abandoned",
        )
    }
}
