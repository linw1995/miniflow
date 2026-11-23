use crate::{
    ChatDatabase, ChatError, Message, Session,
    error::*,
    schema::{messages, model_calls, sessions, turns},
    storage,
};
use diesel::prelude::*;
use mf_runtime::NodeValue;
use snafu::{OptionExt, ResultExt, ensure};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_PAGE_SIZE: usize = 1000;

pub fn default_page_size() -> usize {
    50
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
    #[value(default)]
    pub usages: Option<Vec<crate::CallUsage>>,
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

fn create(input: CreateSessionInputs, provisional: bool) -> Result<SessionOutputs, ChatError> {
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
    let source = if input.title.is_some() && !provisional {
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
        profile: input.profile,
        system: input.system,
        archived: false,
        next_sequence: 0,
        created_at: timestamp,
        updated_at: timestamp,
        context_revision: 0,
    };
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    connection.immediate_transaction(|connection| {
        diesel::insert_into(sessions::table)
            .values(&session)
            .execute(connection)?;
        crate::context::seed(connection, &session)?;
        Ok(SessionOutputs { session })
    })
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
    connection.transaction(|connection| {
        load(connection, &input.session_id)?;
        let after = input.after_sequence.unwrap_or(-1);
        let mut rows: Vec<(Message, String)> = messages::table
            .inner_join(turns::table)
            .filter(messages::session_id.eq(&input.session_id))
            .filter(messages::sequence.gt(after))
            .order(messages::sequence.asc())
            .limit((input.limit + 1) as i64)
            .select((Message::as_select(), turns::state))
            .load(connection)
            .context(DatabaseSnafu {
                operation: "list chat messages",
            })?;
        let next_after = if rows.len() > input.limit {
            rows.truncate(input.limit);
            rows.last().map(|row| row.0.sequence)
        } else {
            None
        };
        let turn_ids: Vec<_> = rows.iter().map(|(message, _)| &message.turn_id).collect();
        let calls = model_calls::table
            .filter(model_calls::session_id.eq(&input.session_id))
            .filter(model_calls::turn_id.eq_any(turn_ids))
            .filter(model_calls::purpose.eq("conversation"))
            .select((model_calls::turn_id, model_calls::usages))
            .load::<(Option<String>, storage::StoredJson<Vec<crate::CallUsage>>)>(connection)
            .context(DatabaseSnafu {
                operation: "load message call facts",
            })?;
        let mut usages: std::collections::BTreeMap<_, _> = calls
            .into_iter()
            .filter_map(|(turn_id, usages)| turn_id.map(|id| (id, Option::from(usages))))
            .collect();
        Ok(MessagePageOutputs {
            page: MessagePage {
                messages: rows
                    .into_iter()
                    .map(|(message, turn_state)| MessageRecord {
                        usages: usages.remove(&message.turn_id).flatten(),
                        message,
                        turn_state,
                    })
                    .collect(),
                next_after,
            },
        })
    })
}

impl ChatDatabase {
    pub fn create_session(
        &self,
        profile: &str,
        system: Option<String>,
        title: Option<String>,
    ) -> Result<Session, ChatError> {
        self.create_session_record(profile, system, title, false)
    }

    /// Creates a replaceable display title rather than a user-assigned name.
    pub fn create_provisional_session(
        &self,
        profile: &str,
        system: Option<String>,
        title: String,
    ) -> Result<Session, ChatError> {
        self.create_session_record(profile, system, Some(title), true)
    }

    fn create_session_record(
        &self,
        profile: &str,
        system: Option<String>,
        title: Option<String>,
        provisional: bool,
    ) -> Result<Session, ChatError> {
        let result: SessionOutputs = create(
            CreateSessionInputs {
                database_path: self
                    .path()
                    .to_str()
                    .context(PathSnafu { path: self.path() })?
                    .into(),
                session_id: uuid::Uuid::new_v4().to_string(),
                profile: profile.into(),
                system,
                title,
            },
            provisional,
        )?;
        Ok(result.session)
    }

    pub fn session(&self, id: &str) -> Result<Session, ChatError> {
        let result: SessionOutputs = get(SessionInputs {
            database_path: self
                .path()
                .to_str()
                .context(PathSnafu { path: self.path() })?
                .into(),
            session_id: id.into(),
        })?;
        Ok(result.session)
    }

    pub fn rename_session(&self, id: &str, title: &str) -> Result<Session, ChatError> {
        let result: SessionOutputs = rename(RenameSessionInputs {
            database_path: self
                .path()
                .to_str()
                .context(PathSnafu { path: self.path() })?
                .into(),
            session_id: id.into(),
            title: title.into(),
        })?;
        Ok(result.session)
    }

    pub fn archive_session(&self, id: &str) -> Result<Session, ChatError> {
        self.change_archive(id, true)
    }
    pub fn restore_session(&self, id: &str) -> Result<Session, ChatError> {
        self.change_archive(id, false)
    }

    fn change_archive(&self, id: &str, archived: bool) -> Result<Session, ChatError> {
        let result = set_archived(
            SessionInputs {
                database_path: self
                    .path()
                    .to_str()
                    .context(PathSnafu { path: self.path() })?
                    .into(),
                session_id: id.into(),
            },
            archived,
        )?;
        Ok(result.session)
    }

    pub fn list_sessions(
        &self,
        archived: bool,
        limit: usize,
        after_id: Option<String>,
    ) -> Result<SessionPage, ChatError> {
        let result: SessionPageOutputs = list(ListSessionsInputs {
            database_path: self
                .path()
                .to_str()
                .context(PathSnafu { path: self.path() })?
                .into(),
            archived,
            limit,
            after_id,
        })?;
        Ok(result.page)
    }

    pub fn messages(
        &self,
        id: &str,
        limit: usize,
        after_sequence: Option<i64>,
    ) -> Result<MessagePage, ChatError> {
        let result: MessagePageOutputs = message_page(MessagesInputs {
            database_path: self
                .path()
                .to_str()
                .context(PathSnafu { path: self.path() })?
                .into(),
            session_id: id.into(),
            limit,
            after_sequence,
        })?;
        Ok(result.page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::connection::SimpleConnection;

    #[test]
    fn session_query_errors_keep_typed_sources() {
        let directory = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
        storage::connect(database.path())
            .unwrap()
            .batch_execute("DROP TABLE sessions;")
            .unwrap();
        let error = database.list_sessions(false, 10, None).unwrap_err();
        assert!(matches!(&error, ChatError::Database { .. }));
        let mut source: Option<&dyn std::error::Error> = Some(&error);
        let mut found = false;
        while let Some(current) = source {
            found |= current.is::<diesel::result::Error>();
            source = current.source();
        }
        assert!(found);
    }

    #[test]
    fn session_apis_preserve_identity_pages_and_archived_data() {
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
    fn history_excludes_unsuccessful_turns_and_has_no_byte_limit() {
        let directory = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
        let session = database
            .create_session("default", Some("instructions".into()), None)
            .unwrap();
        let other = database.create_session("default", None, None).unwrap();
        let identity = crate::ModelIdentity {
            profile: "default".into(),
            model: None,
        };
        let good = database
            .begin_turn(&session.id, "question", identity.clone(), None)
            .unwrap();
        database
            .complete_turn(
                good.invocation,
                crate::ConversationResult::assistant("x".repeat(1048576)),
            )
            .unwrap();
        let bad = database
            .begin_turn(&session.id, "failed question", identity, None)
            .unwrap();
        database
            .fail_turn(bad.invocation, "failed", "failed")
            .unwrap();
        let context = database.model_context(&session.id).unwrap();
        assert_eq!(context.messages.len(), 3);
        assert_eq!(context.revision, 1);
        assert!(
            matches!(&context.messages[0], mfn_llm::ChatMessage::System { content, .. } if content == "instructions")
        );
        assert!(
            matches!(&context.messages[1], mfn_llm::ChatMessage::User { content, .. } if content == "question")
        );
        assert!(
            matches!(&context.messages[2], mfn_llm::ChatMessage::Assistant { content, .. } if content.len() == 1048576)
        );
        assert!(
            database
                .model_context(&other.id)
                .unwrap()
                .messages
                .is_empty()
        );
        let page = database.messages(&session.id, 2, None).unwrap();
        let tail = database.messages(&session.id, 2, page.next_after).unwrap();
        assert_eq!(tail.messages[0].turn_state, "failed");
        assert_eq!(tail.messages[0].message.content(), "failed question");
    }

    #[test]
    fn invalid_session_values_do_not_create_or_rename_records() {
        let directory = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
        let base = CreateSessionInputs {
            database_path: database.path().to_str().unwrap().into(),
            session_id: "session".into(),
            profile: "default".into(),
            system: None,
            title: None,
        };
        let mut id = base.clone();
        id.session_id.clear();
        let mut profile = base.clone();
        profile.profile = " ".into();
        let mut title = base.clone();
        title.title = Some(" ".into());
        for (input, expected) in [(id, "session_id"), (profile, "profile"), (title, "title")] {
            assert!(
                matches!(create(input, false), Err(ChatError::Invalid { field, .. }) if field == expected)
            );
        }
        assert!(
            database
                .list_sessions(false, 10, None)
                .unwrap()
                .sessions
                .is_empty()
        );
        let owner = create(base, false).unwrap().session;
        assert!(matches!(
            rename(RenameSessionInputs {
                database_path: database.path().to_str().unwrap().into(),
                session_id: owner.id.clone(),
                title: " ".into(),
            }),
            Err(ChatError::Invalid { field: "title", .. })
        ));
        assert_eq!(database.session(&owner.id).unwrap().title, owner.title);
        let default = crate::nodes::ModelContextInputs::from_values(mf_runtime::Inputs::from([
            (
                "database_path".into(),
                database.path().to_str().unwrap().into(),
            ),
            ("session_id".into(), owner.id.clone().into()),
        ]))
        .unwrap();
        assert!(
            database
                .model_context(&default.session_id)
                .unwrap()
                .messages
                .is_empty()
        );
    }
}
