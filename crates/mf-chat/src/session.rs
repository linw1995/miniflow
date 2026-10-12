use crate::{
    ChatDatabase, ChatError, Message, Session,
    error::*,
    operation,
    schema::{messages, sessions, turns},
    storage,
};
use diesel::prelude::*;
use mf_runtime::NodeValue;
use snafu::{OptionExt, ResultExt, ensure};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub const DEFAULT_HISTORY_BYTES: usize = 1024 * 1024;
pub const MAX_PAGE_SIZE: usize = 1000;

pub fn default_page_size() -> usize {
    50
}
pub fn default_history_bytes() -> usize {
    DEFAULT_HISTORY_BYTES
}

#[derive(Clone, Debug, NodeValue)]
pub struct CreateSessionInputs {
    pub database_path: String,
    pub session_id: String,
    pub profile: String,
    pub system: Option<String>,
    pub title: Option<String>,
}

#[derive(Clone, Debug, NodeValue)]
pub struct SessionInputs {
    pub database_path: String,
    pub session_id: String,
}

#[derive(Clone, Debug, NodeValue)]
pub struct SessionOutputs {
    pub session: Session,
}

#[derive(Clone, Debug, NodeValue)]
pub struct RenameSessionInputs {
    pub database_path: String,
    pub session_id: String,
    pub title: String,
}

#[derive(Clone, Debug, NodeValue)]
pub struct ListSessionsInputs {
    pub database_path: String,
    #[value(default)]
    pub archived: bool,
    #[value(default = "default_page_size")]
    pub limit: usize,
    pub after_id: Option<String>,
}

#[derive(Clone, Debug, NodeValue)]
pub struct SessionPage {
    pub sessions: Vec<Session>,
    pub next_after: Option<String>,
}

#[derive(Clone, Debug, NodeValue)]
pub struct SessionPageOutputs {
    pub page: SessionPage,
}

#[derive(Clone, Debug, NodeValue)]
pub struct MessagesInputs {
    pub database_path: String,
    pub session_id: String,
    #[value(default = "default_page_size")]
    pub limit: usize,
    pub after_sequence: Option<i64>,
}

#[derive(Clone, Debug, NodeValue)]
pub struct MessageRecord {
    pub message: Message,
    pub turn_state: String,
}

#[derive(Clone, Debug, NodeValue)]
pub struct MessagePage {
    pub messages: Vec<MessageRecord>,
    pub next_after: Option<i64>,
}

#[derive(Clone, Debug, NodeValue)]
pub struct MessagePageOutputs {
    pub page: MessagePage,
}

#[derive(Clone, Debug, NodeValue)]
pub struct HistoryInputs {
    pub database_path: String,
    pub session_id: String,
    #[value(default = "default_history_bytes")]
    pub max_bytes: usize,
}

#[derive(Clone, Debug, NodeValue)]
pub struct HistoryContext {
    pub history: Vec<mfn_llm::ChatMessage>,
    pub system: Option<String>,
}

#[derive(Clone, Debug, NodeValue)]
pub struct HistoryOutputs {
    pub context: HistoryContext,
}

pub fn now() -> Result<i64, ChatError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context(ClockSnafu)?;
    i64::try_from(elapsed.as_millis()).context(TimestampSnafu)
}

pub fn validate_page(limit: usize) -> Result<(), ChatError> {
    ensure!(
        (1..=MAX_PAGE_SIZE).contains(&limit),
        InvalidSnafu {
            field: "limit",
            reason: "must be between 1 and 1000"
        }
    );
    Ok(())
}

pub fn load(connection: &mut SqliteConnection, id: &str) -> Result<Session, ChatError> {
    sessions::table
        .find(id)
        .select(Session::as_select())
        .first(connection)
        .optional()
        .context(DatabaseSnafu {
            operation: "load chat session",
        })?
        .context(SessionMissingSnafu { id })
}

pub fn history(
    connection: &mut SqliteConnection,
    id: &str,
    maximum: usize,
) -> Result<HistoryContext, ChatError> {
    ensure!(
        maximum > 0,
        InvalidSnafu {
            field: "max_bytes",
            reason: "must be positive"
        }
    );
    let session = load(connection, id)?;
    let mut result = Vec::new();
    let mut bytes = 2usize;
    let mut after = -1i64;
    loop {
        let page: Vec<Message> = messages::table
            .inner_join(turns::table)
            .filter(messages::session_id.eq(id))
            .filter(turns::state.eq("succeeded"))
            .filter(messages::sequence.gt(after))
            .order(messages::sequence.asc())
            .limit(100)
            .select(Message::as_select())
            .load(connection)
            .context(DatabaseSnafu {
                operation: "load successful chat history",
            })?;
        if page.is_empty() {
            break;
        }
        for message in page {
            let encoded = serde_json::to_vec(
                &serde_json::json!({"role": message.role, "content": message.content}),
            )
            .context(JsonSnafu)?;
            bytes = bytes
                .checked_add(encoded.len() + usize::from(!result.is_empty()))
                .context(ArithmeticSnafu {
                    operation: "count history bytes",
                })?;
            ensure!(bytes <= maximum, HistoryTooLargeSnafu { limit: maximum });
            after = message.sequence;
            result.push(mfn_llm::ChatMessage {
                role: message.role,
                content: message.content,
            });
        }
    }
    ensure!(bytes <= maximum, HistoryTooLargeSnafu { limit: maximum });
    Ok(HistoryContext {
        history: result,
        system: session.system,
    })
}

fn create(input: CreateSessionInputs) -> Result<SessionOutputs, ChatError> {
    ensure!(
        !input.session_id.is_empty(),
        InvalidSnafu {
            field: "session_id",
            reason: "must be nonempty"
        }
    );
    ensure!(
        !input.profile.trim().is_empty(),
        InvalidSnafu {
            field: "profile",
            reason: "must be nonblank"
        }
    );
    let source = if input.title.is_some() {
        "manual"
    } else {
        "provisional"
    };
    let title = input.title.unwrap_or_else(|| "New conversation".into());
    ensure!(
        !title.trim().is_empty(),
        InvalidSnafu {
            field: "title",
            reason: "must be nonblank"
        }
    );
    let timestamp = now()?;
    let session = Session {
        id: input.session_id,
        title,
        title_source: source.into(),
        title_revision: 0,
        conversation_revision: 0,
        profile: input.profile,
        system: input.system,
        archived: false,
        next_sequence: 0,
        created_at: timestamp,
        updated_at: timestamp,
    };
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    diesel::insert_into(sessions::table)
        .values(&session)
        .execute(&mut connection)?;
    Ok(SessionOutputs { session })
}

fn get(input: SessionInputs) -> Result<SessionOutputs, ChatError> {
    Ok(SessionOutputs {
        session: load(
            &mut storage::connect(Path::new(&input.database_path))?,
            &input.session_id,
        )?,
    })
}

fn rename(input: RenameSessionInputs) -> Result<SessionOutputs, ChatError> {
    ensure!(
        !input.title.trim().is_empty(),
        InvalidSnafu {
            field: "title",
            reason: "must be nonblank"
        }
    );
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    connection.immediate_transaction(|connection| {
        let session = load(connection, &input.session_id)?;
        let revision = session
            .title_revision
            .checked_add(1)
            .context(ArithmeticSnafu {
                operation: "advance title revision",
            })?;
        diesel::update(sessions::table.find(&input.session_id))
            .set((
                sessions::title.eq(&input.title),
                sessions::title_source.eq("manual"),
                sessions::title_revision.eq(revision),
                sessions::updated_at.eq(now()?),
            ))
            .execute(connection)?;
        Ok(SessionOutputs {
            session: load(connection, &input.session_id)?,
        })
    })
}

fn set_archived(input: SessionInputs, archived: bool) -> Result<SessionOutputs, ChatError> {
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    connection.immediate_transaction(|connection| {
        load(connection, &input.session_id)?;
        diesel::update(sessions::table.find(&input.session_id))
            .set((
                sessions::archived.eq(archived),
                sessions::updated_at.eq(now()?),
            ))
            .execute(connection)?;
        Ok(SessionOutputs {
            session: load(connection, &input.session_id)?,
        })
    })
}

fn archive(input: SessionInputs) -> Result<SessionOutputs, ChatError> {
    set_archived(input, true)
}
fn restore(input: SessionInputs) -> Result<SessionOutputs, ChatError> {
    set_archived(input, false)
}

fn list(input: ListSessionsInputs) -> Result<SessionPageOutputs, ChatError> {
    validate_page(input.limit)?;
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    let mut query = sessions::table
        .filter(sessions::archived.eq(input.archived))
        .into_boxed();
    if let Some(id) = &input.after_id {
        let cursor = load(&mut connection, id)?;
        query = query.filter(
            sessions::created_at
                .gt(cursor.created_at)
                .or(sessions::created_at
                    .eq(cursor.created_at)
                    .and(sessions::id.gt(cursor.id))),
        );
    }
    let mut rows = query
        .order((sessions::created_at.asc(), sessions::id.asc()))
        .limit((input.limit + 1) as i64)
        .select(Session::as_select())
        .load::<Session>(&mut connection)
        .context(DatabaseSnafu {
            operation: "list chat sessions",
        })?;
    let next_after = if rows.len() > input.limit {
        rows.truncate(input.limit);
        rows.last().map(|row| row.id.clone())
    } else {
        None
    };
    Ok(SessionPageOutputs {
        page: SessionPage {
            sessions: rows,
            next_after,
        },
    })
}

fn message_page(input: MessagesInputs) -> Result<MessagePageOutputs, ChatError> {
    validate_page(input.limit)?;
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    load(&mut connection, &input.session_id)?;
    let after = input.after_sequence.unwrap_or(-1);
    let mut rows: Vec<(Message, String)> = messages::table
        .inner_join(turns::table)
        .filter(messages::session_id.eq(&input.session_id))
        .filter(messages::sequence.gt(after))
        .order(messages::sequence.asc())
        .limit((input.limit + 1) as i64)
        .select((Message::as_select(), turns::state))
        .load(&mut connection)
        .context(DatabaseSnafu {
            operation: "list chat messages",
        })?;
    let next_after = if rows.len() > input.limit {
        rows.truncate(input.limit);
        rows.last().map(|row| row.0.sequence)
    } else {
        None
    };
    Ok(MessagePageOutputs {
        page: MessagePage {
            messages: rows
                .into_iter()
                .map(|(message, turn_state)| MessageRecord {
                    message,
                    turn_state,
                })
                .collect(),
            next_after,
        },
    })
}

fn load_history(input: HistoryInputs) -> Result<HistoryOutputs, ChatError> {
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    Ok(HistoryOutputs {
        context: connection
            .transaction(|connection| history(connection, &input.session_id, input.max_bytes))?,
    })
}

register_operation!(
    "mf_chat.session.create",
    CreateSession,
    CreateSessionInputs,
    SessionOutputs,
    create
);
register_operation!(
    "mf_chat.session.load",
    LoadSession,
    SessionInputs,
    SessionOutputs,
    get
);
register_operation!(
    "mf_chat.session.rename",
    RenameSession,
    RenameSessionInputs,
    SessionOutputs,
    rename
);
register_operation!(
    "mf_chat.session.archive",
    ArchiveSession,
    SessionInputs,
    SessionOutputs,
    archive
);
register_operation!(
    "mf_chat.session.restore",
    RestoreSession,
    SessionInputs,
    SessionOutputs,
    restore
);
register_operation!(
    "mf_chat.session.list",
    ListSessions,
    ListSessionsInputs,
    SessionPageOutputs,
    list
);
register_operation!(
    "mf_chat.messages.list",
    ListMessages,
    MessagesInputs,
    MessagePageOutputs,
    message_page
);
register_operation!(
    "mf_chat.history.load",
    LoadHistory,
    HistoryInputs,
    HistoryOutputs,
    load_history
);

impl ChatDatabase {
    pub fn create_session(
        &self,
        profile: &str,
        system: Option<String>,
        title: Option<String>,
    ) -> Result<Session, ChatError> {
        let result: SessionOutputs = operation::execute(
            self.path(),
            "mf_chat.session.create",
            CreateSessionInputs {
                database_path: String::new(),
                session_id: uuid::Uuid::new_v4().to_string(),
                profile: profile.into(),
                system,
                title,
            },
        )?;
        Ok(result.session)
    }

    pub fn session(&self, id: &str) -> Result<Session, ChatError> {
        let result: SessionOutputs = operation::execute(
            self.path(),
            "mf_chat.session.load",
            SessionInputs {
                database_path: String::new(),
                session_id: id.into(),
            },
        )?;
        Ok(result.session)
    }

    pub fn rename_session(&self, id: &str, title: &str) -> Result<Session, ChatError> {
        let result: SessionOutputs = operation::execute(
            self.path(),
            "mf_chat.session.rename",
            RenameSessionInputs {
                database_path: String::new(),
                session_id: id.into(),
                title: title.into(),
            },
        )?;
        Ok(result.session)
    }

    pub fn archive_session(&self, id: &str) -> Result<Session, ChatError> {
        self.change_archive(id, true)
    }
    pub fn restore_session(&self, id: &str) -> Result<Session, ChatError> {
        self.change_archive(id, false)
    }

    fn change_archive(&self, id: &str, archived: bool) -> Result<Session, ChatError> {
        let kind = if archived {
            "mf_chat.session.archive"
        } else {
            "mf_chat.session.restore"
        };
        let result: SessionOutputs = operation::execute(
            self.path(),
            kind,
            SessionInputs {
                database_path: String::new(),
                session_id: id.into(),
            },
        )?;
        Ok(result.session)
    }

    pub fn list_sessions(
        &self,
        archived: bool,
        limit: usize,
        after_id: Option<String>,
    ) -> Result<SessionPage, ChatError> {
        let result: SessionPageOutputs = operation::execute(
            self.path(),
            "mf_chat.session.list",
            ListSessionsInputs {
                database_path: String::new(),
                archived,
                limit,
                after_id,
            },
        )?;
        Ok(result.page)
    }

    pub fn messages(
        &self,
        id: &str,
        limit: usize,
        after_sequence: Option<i64>,
    ) -> Result<MessagePage, ChatError> {
        let result: MessagePageOutputs = operation::execute(
            self.path(),
            "mf_chat.messages.list",
            MessagesInputs {
                database_path: String::new(),
                session_id: id.into(),
                limit,
                after_sequence,
            },
        )?;
        Ok(result.page)
    }

    pub fn history(&self, id: &str, max_bytes: usize) -> Result<HistoryContext, ChatError> {
        let result: HistoryOutputs = operation::execute(
            self.path(),
            "mf_chat.history.load",
            HistoryInputs {
                database_path: String::new(),
                session_id: id.into(),
                max_bytes,
            },
        )?;
        Ok(result.context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::connection::SimpleConnection;

    #[test]
    fn preparation_is_pure_and_query_errors_keep_their_sources() {
        let directory = tempfile::tempdir().unwrap();
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        for kind in [
            "mf_chat.session.create",
            "mf_chat.session.load",
            "mf_chat.session.list",
            "mf_chat.session.rename",
            "mf_chat.session.archive",
            "mf_chat.session.restore",
            "mf_chat.messages.list",
            "mf_chat.history.load",
        ] {
            registry
                .get(kind)
                .unwrap()
                .instantiate(serde_json::json!({}))
                .unwrap();
        }
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
        storage::connect(database.path())
            .unwrap()
            .batch_execute("DROP TABLE sessions;")
            .unwrap();
        let error = database.list_sessions(false, 10, None).unwrap_err();
        let mut source: Option<&dyn std::error::Error> = Some(&error);
        let mut found = false;
        while let Some(current) = source {
            found |= current.is::<diesel::result::Error>();
            source = current.source();
        }
        assert!(found);
    }

    #[test]
    fn session_nodes_preserve_identity_pages_and_archived_data() {
        let directory = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
        let first = database
            .create_session("profile-a", Some("system".into()), None)
            .unwrap();
        let second = database
            .create_session("profile-b", None, Some("Manual".into()))
            .unwrap();
        let page = database.list_sessions(false, 1, None).unwrap();
        let tail = database.list_sessions(false, 1, page.next_after).unwrap();
        assert_eq!(page.sessions.len(), 1);
        assert_eq!(tail.sessions.len(), 1);
        assert_ne!(page.sessions[0].id, tail.sessions[0].id);
        assert!(tail.next_after.is_none());
        let renamed = database.rename_session(&first.id, "Renamed").unwrap();
        assert_eq!(renamed.title_source, "manual");
        assert_eq!(renamed.title_revision, 1);
        assert!(database.archive_session(&first.id).unwrap().archived);
        assert_eq!(
            database.list_sessions(false, 10, None).unwrap().sessions[0].id,
            second.id
        );
        assert_eq!(
            database.list_sessions(true, 10, None).unwrap().sessions[0].id,
            first.id
        );
        assert!(!database.restore_session(&first.id).unwrap().archived);
        assert!(database.session("missing").is_err());
        assert!(database.list_sessions(false, 0, None).is_err());
    }

    #[test]
    fn history_excludes_unsuccessful_turns_and_limits_encoded_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
        let session = database
            .create_session("default", Some("instructions".into()), None)
            .unwrap();
        let other = database.create_session("default", None, None).unwrap();
        let mut connection = storage::connect(database.path()).unwrap();
        connection
            .batch_execute(&format!(
                "INSERT INTO turns VALUES ('good', '{}', NULL, 0, 'succeeded', 1, 2, NULL);
             INSERT INTO turns VALUES ('bad', '{}', NULL, 2, 'failed', 3, 4, 'failed');
             INSERT INTO messages VALUES ('u', '{}', 'good', 0, 'user', 'question', 1);
             INSERT INTO messages VALUES ('a', '{}', 'good', 1, 'assistant', 'answer', 2);
             INSERT INTO messages VALUES ('b', '{}', 'bad', 2, 'user', 'failed question', 3);",
                session.id, session.id, session.id, session.id, session.id
            ))
            .unwrap();
        let context = database.history(&session.id, 1024).unwrap();
        assert_eq!(context.history.len(), 2);
        assert_eq!(context.history[0].content, "question");
        assert_eq!(context.history[1].content, "answer");
        assert_eq!(context.system.as_deref(), Some("instructions"));
        assert!(database.history(&session.id, 2).is_err());
        assert!(
            database
                .history(&other.id, 1024)
                .unwrap()
                .history
                .is_empty()
        );
        let page = database.messages(&session.id, 2, None).unwrap();
        let tail = database.messages(&session.id, 2, page.next_after).unwrap();
        assert_eq!(tail.messages[0].turn_state, "failed");
        assert_eq!(tail.messages[0].message.content, "failed question");
    }
}
