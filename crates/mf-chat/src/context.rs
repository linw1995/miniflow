use crate::{
    ChatDatabase, ChatError, ConversationResult, Message, MessageUpdate, Session, Turn,
    error::*,
    schema::{context_revisions, messages, sessions},
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

register_operation!(
    "mf_chat.context.load",
    LoadModelContext,
    ModelContextInputs,
    ModelContextOutputs,
    load
);

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
}
