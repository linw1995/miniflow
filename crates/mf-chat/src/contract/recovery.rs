//! Bounded terminal results retained by the owning application for save-only retry.

use super::{InvocationState, TurnExit};
use crate::{ChatDatabase, ChatError, Completion, Invocation, error::*};
use snafu::{IntoError, ensure};

pub const MAX_RECOVERY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct UncommittedCompletion {
    pub database_path: std::path::PathBuf,
    pub completion: Completion,
}

#[derive(Clone, Debug)]
pub enum CompletionState {
    NotBegun,
    /// The complete terminal result is durable, whether its business status succeeded or failed.
    Committed(Completion),
    Uncommitted(UncommittedCompletion),
    Finished(crate::Turn),
}

pub struct ConversationExit<'a> {
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
        self.reconcile_result(
            exit.invocation,
            exit.stdout.map(crate::ConversationResult::from_json),
            exit.outcome,
            exit.summary,
        )
    }

    pub fn reconcile_result(
        &self,
        invocation: &Invocation,
        result: Option<Result<crate::ConversationResult, ChatError>>,
        outcome: TurnExit,
        summary: &str,
    ) -> Result<ReconciledCompletion, ChatError> {
        let stored = self.inspect_invocation(invocation)?;
        let mut diagnostics = Vec::new();
        let delivered = result.map(|result| {
            result.and_then(|result| {
                self.validate_completion(invocation, &result)?;
                Ok(Completion {
                    invocation: invocation.clone(),
                    result,
                })
            })
        });
        if let InvocationState::Committed(saved) = stored {
            match delivered {
                Some(Ok(completion)) if completion != saved => diagnostics.push(
                    InvalidSnafu {
                        field: "workflow result",
                        reason: "disagrees with the committed terminal result",
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
                    }),
                    diagnostics,
                });
            }
            Some(Err(source)) => diagnostics.push(source),
            Some(Ok(_)) => diagnostics.push(
                InvalidSnafu {
                    field: "workflow result",
                    reason: "requires the parent's admitted running invocation",
                }
                .build(),
            ),
            None => {}
        }
        let state = match self.finalize_invocation(invocation.clone(), outcome, summary)? {
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
            pending.database_path == self.path(),
            InvalidSnafu {
                field: "database_path",
                reason: "completion belongs to another host database"
            }
        );
        match self.complete_turn(
            pending.completion.invocation.clone(),
            pending.completion.result.clone(),
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
            pending.database_path == self.path(),
            InvalidSnafu {
                field: "retained completion",
                reason: "belongs to another database"
            }
        );
        self.finalize_invocation(
            pending.completion.invocation.clone(),
            TurnExit::Cancelled,
            "uncommitted terminal result abandoned",
        )
    }
}
