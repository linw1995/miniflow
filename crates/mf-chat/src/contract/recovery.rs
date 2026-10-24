//! Bounded model replies retained by the owning application for save-only retry.

use super::{InvocationState, TurnExit};
use crate::{ChatDatabase, ChatError, Completion, Invocation, error::*, turn};
use snafu::{IntoError, ensure};

pub const MAX_RECOVERY_BYTES: usize = 16 * 1024 * 1024;

enum Delivered {
    Succeeded(Completion),
    Failed {
        error: String,
        usages: Vec<crate::CallUsage>,
    },
}

fn delivered(
    mut result: crate::ConversationResult,
    invocation: &Invocation,
) -> Result<Delivered, ChatError> {
    result.validate()?;
    if result.status == "succeeded" {
        result.error = None;
        Ok(Delivered::Succeeded(Completion {
            invocation: invocation.clone(),
            result,
        }))
    } else {
        Ok(Delivered::Failed {
            error: result.error.expect("validated failure has an error"),
            usages: result.usages,
        })
    }
}

#[derive(Clone, Debug)]
pub struct UncommittedCompletion {
    pub database_path: std::path::PathBuf,
    pub completion: Completion,
}

#[derive(Clone, Debug)]
pub enum CompletionState {
    NotBegun,
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
        let delivered =
            result.map(|result| result.and_then(|result| delivered(result, invocation)));
        if let InvocationState::Committed(saved) = stored {
            match delivered {
                Some(Ok(Delivered::Succeeded(completion))) if completion != saved => diagnostics
                    .push(
                        InvalidSnafu {
                            field: "model reply",
                            reason: "disagrees with the committed reply",
                        }
                        .build(),
                    ),
                Some(Ok(Delivered::Failed { .. })) => diagnostics.push(
                    InvalidSnafu {
                        field: "workflow result",
                        reason: "failure arrived after committed success",
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
            Some(Ok(Delivered::Succeeded(completion)))
                if matches!(stored, InvocationState::Running) =>
            {
                return Ok(ReconciledCompletion {
                    state: CompletionState::Uncommitted(UncommittedCompletion {
                        database_path: self.path().to_owned(),
                        completion,
                    }),
                    diagnostics,
                });
            }
            Some(Ok(Delivered::Failed { error, usages }))
                if matches!(stored, InvocationState::Running) =>
            {
                self.fail_turn_with_usages(invocation.clone(), &error, usages)?;
            }
            Some(Ok(Delivered::Failed { error, usages })) => {
                if let InvocationState::Finished(turn) = &stored
                    && turn.state == "failed"
                {
                    let call = self.call(&invocation.call_id)?;
                    if call.usages.as_deref() == Some(usages.as_slice())
                        && call.error.as_deref() == Some(turn::failure_summary(&error))
                    {
                        return Ok(ReconciledCompletion {
                            state: CompletionState::Finished(turn.clone()),
                            diagnostics,
                        });
                    }
                }
                diagnostics.push(
                    InvalidSnafu {
                        field: "workflow result",
                        reason: "disagrees with the finished execution",
                    }
                    .build(),
                );
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
        let finalized = self.finalize_invocation(invocation.clone(), outcome, summary)?;
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
            "uncommitted reply abandoned",
        )
    }
}
