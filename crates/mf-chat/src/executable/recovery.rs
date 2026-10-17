//! Bounded model replies retained by the owning application for save-only retry.

use super::{ChatContract, InvocationState, TurnExit};
use crate::{
    CallPurpose, ChatDatabase, ChatError, Completion, Invocation, UncommittedCompletion, error::*,
    turn,
};
use mf_runtime::NodeValue;
use snafu::{IntoError, OptionExt, ResultExt, ensure};

pub const MAX_RECOVERY_BYTES: usize = 16 * 1024 * 1024;

#[derive(NodeValue)]
struct WorkflowResult {
    status: String,
    text: Option<String>,
    metadata: Option<mfn_llm::MessageMetadata>,
    usages: Vec<mfn_llm::CallUsage>,
    finish_reason: Option<String>,
    error: Option<String>,
}

enum Delivered {
    Succeeded(Completion),
    Failed {
        error: String,
        usages: Vec<mfn_llm::CallUsage>,
    },
}

impl ChatContract {
    pub fn decode_completion(
        &self,
        bytes: &[u8],
        invocation: &Invocation,
    ) -> Result<Completion, ChatError> {
        match self.decode_result(bytes, invocation)? {
            Delivered::Succeeded(completion) => Ok(completion),
            Delivered::Failed { .. } => InvalidSnafu {
                field: "workflow result",
                reason: "workflow reported failure",
            }
            .fail(),
        }
    }

    fn decode_result(&self, bytes: &[u8], invocation: &Invocation) -> Result<Delivered, ChatError> {
        ensure!(
            bytes.len() <= MAX_RECOVERY_BYTES,
            RecoveryTooLargeSnafu {
                limit: MAX_RECOVERY_BYTES
            }
        );
        let mut values: mf_runtime::NodeValues =
            mf_runtime::decode_unique_json(bytes).context(JsonSnafu)?;
        values.retain(|name, _| {
            matches!(
                name.as_str(),
                "status" | "text" | "metadata" | "usages" | "finish_reason" | "error"
            )
        });
        let result = WorkflowResult::from_values(values)?;
        turn::validate_usages(&result.usages)?;
        match result.status.as_str() {
            "succeeded" => {
                ensure!(
                    result.error.as_ref().is_none_or(|error| error.is_empty()),
                    InvalidSnafu {
                        field: "workflow result",
                        reason: "success cannot include an error"
                    }
                );
                let text = result.text.context(InvalidSnafu {
                    field: "workflow result",
                    reason: "success requires text",
                })?;
                let reply = crate::Reply {
                    text,
                    metadata: result.metadata,
                    usages: Some(result.usages),
                    usage: None,
                    finish_reason: result.finish_reason,
                };
                turn::validate_reply(&reply)?;
                Ok(Delivered::Succeeded(Completion {
                    invocation: invocation.clone(),
                    reply,
                }))
            }
            "failed" => {
                let error = result
                    .error
                    .filter(|error| !error.trim().is_empty())
                    .context(InvalidSnafu {
                        field: "workflow result",
                        reason: "failure requires an error summary",
                    })?;
                Ok(Delivered::Failed {
                    error,
                    usages: result.usages,
                })
            }
            _ => InvalidSnafu {
                field: "workflow result",
                reason: "requires succeeded or failed status",
            }
            .fail(),
        }
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
            .map(|bytes| exit.contract.decode_result(bytes, exit.invocation));
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
                        purpose: CallPurpose::Conversation,
                    }),
                    diagnostics,
                });
            }
            Some(Ok(Delivered::Failed { error, usages }))
                if matches!(stored, InvocationState::Running) =>
            {
                self.fail_turn_with_usages(exit.invocation.clone(), &error, usages)?;
            }
            Some(Ok(Delivered::Failed { error, usages })) => {
                if let InvocationState::Finished(turn) = &stored
                    && turn.state == "failed"
                {
                    let call = self.call(&exit.invocation.call_id)?;
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
