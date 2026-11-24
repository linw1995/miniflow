use crate::{
    CallUsage, ChatDatabase, ChatError, ConversationResult, Message, MessageUpdate, Session, Turn,
    error::*,
    schema::{context_revisions, messages, model_calls, sessions},
    session, storage,
};
use diesel::prelude::*;
use mf_runtime::NodeValue;
use mfn_llm::ChatMessage;
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, ensure};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct ModelContext {
    pub revision: i64,
    pub messages: Vec<ChatMessage>,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = context_revisions)]
struct Revision {
    session_id: String,
    revision: i64,
    parent_revision: Option<i64>,
    turn_id: Option<String>,
    operation: String,
    #[diesel(deserialize_as = storage::StoredJson<Vec<ChatMessage>>)]
    snapshot: Option<Vec<ChatMessage>>,
}

pub fn seed(connection: &mut SqliteConnection, owner: &Session) -> Result<(), ChatError> {
    let initial = owner
        .system
        .as_ref()
        .filter(|system| !system.is_empty())
        .map(|content| ChatMessage::System {
            content: content.clone(),
            metadata: None,
        })
        .into_iter()
        .collect::<Vec<_>>();
    diesel::insert_into(context_revisions::table)
        .values((
            context_revisions::session_id.eq(&owner.id),
            context_revisions::revision.eq(0),
            context_revisions::operation.eq("checkpoint"),
            context_revisions::snapshot.eq(storage::StoredJson::from(Some(initial))),
        ))
        .execute(connection)
        .context(DatabaseSnafu {
            operation: "seed model context",
        })?;
    Ok(())
}

pub fn at(
    connection: &mut SqliteConnection,
    id: &str,
    revision: i64,
) -> Result<ModelContext, ChatError> {
    let mut cursor = revision;
    let mut increments = Vec::new();
    let mut snapshot;
    loop {
        let record = context_revisions::table
            .find((id, cursor))
            .select(Revision::as_select())
            .first::<Revision>(connection)
            .optional()
            .context(DatabaseSnafu {
                operation: "load model context revision",
            })?
            .context(ContextMissingSnafu {
                session_id: id,
                revision: cursor,
            })?;
        ensure!(
            record.session_id == id && record.revision == cursor,
            InvalidSnafu {
                field: "model context",
                reason: "revision ownership mismatch"
            }
        );
        if record.operation == "checkpoint" {
            snapshot = record.snapshot.context(InvalidSnafu {
                field: "model context",
                reason: "checkpoint requires a snapshot",
            })?;
            break;
        }
        ensure!(
            record.operation == "append" && record.snapshot.is_none(),
            InvalidSnafu {
                field: "model context",
                reason: "requires an append or checkpoint revision"
            }
        );
        let turn_id = record.turn_id.context(InvalidSnafu {
            field: "model context",
            reason: "append requires an originating turn",
        })?;
        let turn = crate::turn::load_turn(connection, &turn_id)?;
        ensure!(
            turn.session_id == id && turn.state == "succeeded",
            InvalidSnafu {
                field: "model context",
                reason: "append requires a successful originating turn"
            }
        );
        let increment = messages::table
            .filter(messages::session_id.eq(id))
            .filter(messages::turn_id.eq(&turn_id))
            .order(messages::sequence.asc())
            .select(Message::as_select())
            .load::<Message>(connection)
            .context(DatabaseSnafu {
                operation: "load model context increment",
            })?
            .into_iter()
            .map(|message| message.payload)
            .collect::<Vec<_>>();
        increments.push(increment);
        let parent = record.parent_revision.context(InvalidSnafu {
            field: "model context",
            reason: "append requires a parent revision",
        })?;
        ensure!(
            parent >= 0 && parent < cursor,
            InvalidSnafu {
                field: "model context",
                reason: "parent must precede its child revision"
            }
        );
        cursor = parent;
    }
    for increment in increments.into_iter().rev() {
        snapshot.extend(increment);
    }
    mfn_llm::validate_transcript(&snapshot, mfn_llm::TranscriptMode::Complete).context(
        TranscriptSnafu {
            view: "stored model context",
        },
    )?;
    Ok(ModelContext {
        revision,
        messages: snapshot,
    })
}

pub fn current(connection: &mut SqliteConnection, id: &str) -> Result<ModelContext, ChatError> {
    let owner = session::load(connection, id)?;
    at(connection, id, owner.context_revision)
}

pub fn publish(
    connection: &mut SqliteConnection,
    owner: &Session,
    turn: &Turn,
    result: &ConversationResult,
) -> Result<(), ChatError> {
    ensure!(
        owner.context_revision == turn.context_revision,
        ContextConflictSnafu {
            session_id: &owner.id,
            expected: turn.context_revision,
            actual: owner.context_revision
        }
    );
    let revision = owner
        .context_revision
        .checked_add(1)
        .context(ArithmeticSnafu {
            operation: "advance model context revision",
        })?;
    let (operation, snapshot) = match &result.update {
        MessageUpdate::Append { .. } => ("append", None),
        MessageUpdate::ReplaceContext {
            context_messages, ..
        } => ("checkpoint", Some(context_messages.clone())),
    };
    diesel::insert_into(context_revisions::table)
        .values((
            context_revisions::session_id.eq(&owner.id),
            context_revisions::revision.eq(revision),
            context_revisions::parent_revision.eq(Some(turn.context_revision)),
            context_revisions::turn_id.eq(Some(&turn.id)),
            context_revisions::operation.eq(operation),
            context_revisions::snapshot.eq(storage::StoredJson::from(snapshot)),
        ))
        .execute(connection)
        .context(DatabaseSnafu {
            operation: "save model context revision",
        })?;
    let changed = diesel::update(
        sessions::table
            .find(&owner.id)
            .filter(sessions::context_revision.eq(turn.context_revision)),
    )
    .set(sessions::context_revision.eq(revision))
    .execute(connection)
    .context(DatabaseSnafu {
        operation: "publish model context revision",
    })?;
    ensure!(
        changed == 1,
        ContextConflictSnafu {
            session_id: &owner.id,
            expected: turn.context_revision,
            actual: owner.context_revision
        }
    );
    Ok(())
}

#[derive(Clone, Debug, NodeValue)]
pub struct ModelContextInputs {
    pub database_path: String,
    pub session_id: String,
}
#[derive(Clone, Debug, NodeValue)]
pub struct ModelContextOutputs {
    pub context: ModelContext,
}

fn load(input: ModelContextInputs) -> Result<ModelContextOutputs, ChatError> {
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    Ok(ModelContextOutputs {
        context: connection.transaction(|connection| current(connection, &input.session_id))?,
    })
}

/// Returns the context token count reported by the last valid non-auxiliary model call.
pub fn context_tokens(usages: &[CallUsage]) -> Option<u64> {
    usages.iter().rev().find_map(call_context_tokens)
}

fn call_context_tokens(call: &CallUsage) -> Option<u64> {
    if call.error.is_some() {
        return None;
    }
    if call
        .attribution
        .as_ref()
        .and_then(|attribution| attribution.purpose.as_deref())
        .is_some_and(|purpose| matches!(purpose, "title" | "compaction"))
    {
        return None;
    }
    let usage = call.usage.as_ref()?;
    if let Some(total) = usage
        .get("total_tokens")
        .copied()
        .and_then(|value| u64::try_from(value).ok())
        .filter(|&value| value > 0)
    {
        return Some(total);
    }
    let sum = [
        "input_tokens",
        "output_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
    ]
    .into_iter()
    .filter_map(|key| {
        usage
            .get(key)
            .copied()
            .and_then(|value| u64::try_from(value).ok())
    })
    .fold(0_u64, u64::saturating_add);
    (sum > 0).then_some(sum)
}

fn current_tokens(connection: &mut SqliteConnection, id: &str) -> Result<Option<u64>, ChatError> {
    let owner = session::load(connection, id)?;
    if owner.context_revision == 0 {
        return Ok(Some(0));
    }
    let mut cursor = owner.context_revision;
    loop {
        // Estimating usage must not deserialize a checkpoint's message snapshot.
        let (operation, turn_id, parent) = context_revisions::table
            .find((id, cursor))
            .select((
                context_revisions::operation,
                context_revisions::turn_id,
                context_revisions::parent_revision,
            ))
            .first::<(String, Option<String>, Option<i64>)>(connection)
            .optional()
            .context(DatabaseSnafu {
                operation: "load model context revision header",
            })?
            .context(ContextMissingSnafu {
                session_id: id,
                revision: cursor,
            })?;
        if operation == "checkpoint" {
            // Do not carry pre-compaction usage across a context replacement boundary.
            return Ok(None);
        }
        let turn_id = turn_id.context(InvalidSnafu {
            field: "model context",
            reason: "append revision requires a turn identifier",
        })?;
        let calls: Vec<storage::StoredJson<Vec<CallUsage>>> = model_calls::table
            .filter(model_calls::session_id.eq(id))
            .filter(model_calls::turn_id.eq(&turn_id))
            .filter(model_calls::purpose.eq("conversation"))
            .order(model_calls::finished_at.desc())
            .select(model_calls::usages)
            .load(connection)
            .context(DatabaseSnafu {
                operation: "load turn usages for context estimate",
            })?;
        for usages in calls {
            if let Some(usages) = Option::<Vec<CallUsage>>::from(usages)
                && let Some(tokens) = context_tokens(&usages)
            {
                return Ok(Some(tokens));
            }
        }
        let parent = parent.context(InvalidSnafu {
            field: "model context",
            reason: "append requires a parent revision",
        })?;
        ensure!(
            parent >= 0 && parent < cursor,
            InvalidSnafu {
                field: "model context",
                reason: "parent must precede its child revision"
            }
        );
        cursor = parent;
    }
}

impl ChatDatabase {
    pub fn model_context(&self, session_id: &str) -> Result<ModelContext, ChatError> {
        Ok(load(ModelContextInputs {
            database_path: self
                .path()
                .to_str()
                .context(PathSnafu { path: self.path() })?
                .into(),
            session_id: session_id.into(),
        })?
        .context)
    }

    /// Returns the active context token count for `session_id` since the latest checkpoint.
    ///
    /// Returns `Some(0)` for an empty session at revision 0, `Some(tokens)` for the latest valid
    /// non-auxiliary usage after the last checkpoint, and `None` when a checkpoint replacement
    /// (such as compaction) has no subsequent usage or no provider usage was reported.
    pub fn context_tokens(&self, session_id: &str) -> Result<Option<u64>, ChatError> {
        let mut connection = storage::connect(self.path())?;
        connection.transaction(|connection| current_tokens(connection, session_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMessage, ModelIdentity, UsageAttribution};
    use std::collections::BTreeMap;

    fn usage_entry(
        call_id: &str,
        purpose: Option<&str>,
        counters: &[(&str, i64)],
        error: Option<&str>,
    ) -> CallUsage {
        CallUsage {
            attribution: purpose.map(|purpose| UsageAttribution {
                purpose: Some(purpose.into()),
                profile: None,
            }),
            started_at: None,
            timing: None,
            call_id: call_id.into(),
            node: None,
            model: "test-model".into(),
            usage: (!counters.is_empty()).then(|| {
                counters
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), *v))
                    .collect::<BTreeMap<_, _>>()
            }),
            error: error.map(str::to_owned),
        }
    }

    #[test]
    fn context_tokens_picks_last_valid_non_auxiliary_usage() {
        let usages = vec![
            usage_entry("c1", None, &[("total_tokens", 12_000)], None),
            usage_entry(
                "c2",
                None,
                &[
                    ("input_tokens", 15_000),
                    ("output_tokens", 500),
                    ("cache_read_input_tokens", 2_000),
                ],
                None,
            ),
            usage_entry("c3", Some("title"), &[("total_tokens", 99_000)], None),
            usage_entry("c4", Some("compaction"), &[("total_tokens", 88_000)], None),
            usage_entry("c5", None, &[("total_tokens", 77_000)], Some("timeout")),
        ];
        assert_eq!(context_tokens(&usages), Some(17_500));
        assert_eq!(
            context_tokens(&[usage_entry(
                "c6",
                Some("title"),
                &[("total_tokens", 500)],
                None
            )]),
            None
        );
    }

    #[test]
    fn database_context_tokens_tracks_appends_and_resets_after_context_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let db = ChatDatabase::open(dir.path().join("chat.sqlite")).unwrap();
        let session = db
            .create_session("default", None, Some("Context test".into()))
            .unwrap();
        let model = || ModelIdentity {
            profile: "default".into(),
            model: None,
        };

        // New empty session reports 0 tokens.
        assert_eq!(db.context_tokens(&session.id).unwrap(), Some(0));

        // First append turn reports 307,000 total tokens.
        let mut reply1 = ConversationResult::assistant("first answer");
        reply1.usages = vec![
            usage_entry("call-1a", None, &[("total_tokens", 100_000)], None),
            usage_entry("call-1b", None, &[("total_tokens", 307_000)], None),
            usage_entry("call-title", Some("title"), &[("total_tokens", 400)], None),
        ];
        let started1 = db.begin_turn(&session.id, "hello", model(), None).unwrap();
        db.complete_turn(started1.invocation, reply1).unwrap();
        assert_eq!(db.context_tokens(&session.id).unwrap(), Some(307_000));

        // Second append turn without usage falls back to prior append turn since same checkpoint.
        let started2 = db
            .begin_turn(&session.id, "follow up", model(), None)
            .unwrap();
        db.complete_turn(
            started2.invocation,
            ConversationResult::assistant("no usage reply"),
        )
        .unwrap();
        assert_eq!(db.context_tokens(&session.id).unwrap(), Some(307_000));

        // Context replacement (compaction) invalidates stale pre-compaction token usage.
        let compacted = ConversationResult {
            status: "succeeded".into(),
            update: MessageUpdate::ReplaceContext {
                display_messages: vec![ChatMessage::Assistant {
                    content: "compacted answer".into(),
                    tool_calls: None,
                    metadata: None,
                }],
                context_messages: vec![
                    ChatMessage::User {
                        content: "summary".into(),
                        metadata: None,
                    },
                    ChatMessage::Assistant {
                        content: "compacted answer".into(),
                        tool_calls: None,
                        metadata: None,
                    },
                ],
            },
            usages: vec![
                usage_entry("call-3-main", None, &[("total_tokens", 450_000)], None),
                usage_entry(
                    "call-3-compact",
                    Some("compaction"),
                    &[("total_tokens", 460_000)],
                    None,
                ),
            ],
            title: None,
            error: None,
        };
        let started3 = db
            .begin_turn(&session.id, "compact now", model(), None)
            .unwrap();
        db.complete_turn(started3.invocation, compacted).unwrap();
        assert_eq!(db.context_tokens(&session.id).unwrap(), None);

        // Next post-compaction append turn restores fresh accurate token usage.
        let mut reply4 = ConversationResult::assistant("post-compact answer");
        reply4.usages = vec![usage_entry(
            "call-4a",
            None,
            &[("total_tokens", 28_500)],
            None,
        )];
        let started4 = db
            .begin_turn(&session.id, "after compaction", model(), None)
            .unwrap();
        db.complete_turn(started4.invocation, reply4).unwrap();
        assert_eq!(db.context_tokens(&session.id).unwrap(), Some(28_500));
    }
}
