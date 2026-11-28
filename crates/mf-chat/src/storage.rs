use crate::{ChatError, error::*};
use diesel::{Connection, connection::SimpleConnection, sqlite::SqliteConnection};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use fs2::FileExt;
use snafu::{IntoError, OptionExt, ResultExt};
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

#[derive(Debug, diesel::expression::AsExpression, diesel::deserialize::FromSqlRow)]
#[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
pub struct StoredJson<T>(Option<T>);

impl<T> From<Option<T>> for StoredJson<T> {
    fn from(value: Option<T>) -> Self {
        Self(value)
    }
}

impl<T> From<StoredJson<T>> for Option<T> {
    fn from(value: StoredJson<T>) -> Self {
        value.0
    }
}

impl<T: serde::de::DeserializeOwned>
    diesel::deserialize::FromSql<
        diesel::sql_types::Nullable<diesel::sql_types::Text>,
        diesel::sqlite::Sqlite,
    > for StoredJson<T>
{
    fn from_sql(
        value: diesel::sqlite::SqliteValue<'_, '_, '_>,
    ) -> diesel::deserialize::Result<Self> {
        Self::from_nullable_sql(Some(value))
    }

    fn from_nullable_sql(
        value: Option<diesel::sqlite::SqliteValue<'_, '_, '_>>,
    ) -> diesel::deserialize::Result<Self> {
        let encoded = <Option<String> as diesel::deserialize::FromSql<
            diesel::sql_types::Nullable<diesel::sql_types::Text>,
            diesel::sqlite::Sqlite,
        >>::from_nullable_sql(value)?;
        Ok(Self(
            encoded
                .map(|value| mf_runtime::decode_unique_json(value.as_bytes()))
                .transpose()?,
        ))
    }
}

impl<T: serde::Serialize + std::fmt::Debug>
    diesel::serialize::ToSql<
        diesel::sql_types::Nullable<diesel::sql_types::Text>,
        diesel::sqlite::Sqlite,
    > for StoredJson<T>
{
    fn to_sql<'b>(
        &'b self,
        output: &mut diesel::serialize::Output<'b, '_, diesel::sqlite::Sqlite>,
    ) -> diesel::serialize::Result {
        match &self.0 {
            Some(value) => {
                output.set_value(serde_json::to_string(value)?);
                Ok(diesel::serialize::IsNull::No)
            }
            None => Ok(diesel::serialize::IsNull::Yes),
        }
    }
}

#[derive(Debug, diesel::expression::AsExpression, diesel::deserialize::FromSqlRow)]
#[diesel(sql_type = diesel::sql_types::Text)]
pub struct StoredMessage(mfn_llm::ChatMessage);

impl From<mfn_llm::ChatMessage> for StoredMessage {
    fn from(value: mfn_llm::ChatMessage) -> Self {
        Self(value)
    }
}

impl From<StoredMessage> for mfn_llm::ChatMessage {
    fn from(value: StoredMessage) -> Self {
        value.0
    }
}

impl diesel::deserialize::FromSql<diesel::sql_types::Text, diesel::sqlite::Sqlite>
    for StoredMessage
{
    fn from_sql(
        value: diesel::sqlite::SqliteValue<'_, '_, '_>,
    ) -> diesel::deserialize::Result<Self> {
        let encoded = <String as diesel::deserialize::FromSql<
            diesel::sql_types::Text,
            diesel::sqlite::Sqlite,
        >>::from_sql(value)?;
        Ok(Self(mf_runtime::decode_unique_json(encoded.as_bytes())?))
    }
}

impl diesel::serialize::ToSql<diesel::sql_types::Text, diesel::sqlite::Sqlite> for StoredMessage {
    fn to_sql<'b>(
        &'b self,
        output: &mut diesel::serialize::Output<'b, '_, diesel::sqlite::Sqlite>,
    ) -> diesel::serialize::Result {
        output.set_value(serde_json::to_string(&self.0)?);
        Ok(diesel::serialize::IsNull::No)
    }
}

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");
const CONNECTION_PRAGMAS: &str = "PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;";

/// Owns initialization and recovery rights for one database until dropped.
pub struct ChatDatabase {
    pub(crate) deliveries: std::sync::Arc<crate::steering::Deliveries>,
    path: PathBuf,
    _ownership: File,
}

impl ChatDatabase {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ChatError> {
        let supplied = path.as_ref();
        let parent = supplied
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent).context(IoSnafu {
            operation: "create database directory",
            path: parent,
        })?;
        let parent = fs::canonicalize(parent).context(IoSnafu {
            operation: "resolve database directory",
            path: parent,
        })?;
        let name = supplied.file_name().context(PathSnafu { path: supplied })?;
        let path = if supplied.exists() {
            fs::canonicalize(supplied).context(IoSnafu {
                operation: "resolve database path",
                path: supplied,
            })?
        } else {
            parent.join(name)
        };
        let mut lock_name = path.as_os_str().to_os_string();
        lock_name.push(".lock");
        let lock_path = PathBuf::from(lock_name);
        let ownership = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .context(IoSnafu {
                operation: "open ownership lock",
                path: &lock_path,
            })?;
        match ownership.try_lock_exclusive() {
            Ok(()) => {}
            Err(source) if source.kind() == io::ErrorKind::WouldBlock => {
                return BusySnafu { path }.fail();
            }
            Err(source) => {
                return Err(IoSnafu {
                    operation: "lock database ownership",
                    path: lock_path,
                }
                .into_error(source));
            }
        }
        initialize(&path)?;
        let database = Self {
            deliveries: Default::default(),
            path,
            _ownership: ownership,
        };
        let _: crate::nodes::RecoveryOutputs =
            crate::turn::recover(crate::nodes::RecoveryInputs {
                database_path: database
                    .path()
                    .to_str()
                    .context(PathSnafu {
                        path: database.path(),
                    })?
                    .into(),
            })?;
        Ok(database)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn connect(path: &Path) -> Result<SqliteConnection, ChatError> {
    let location = path.to_str().context(PathSnafu { path })?;
    let mut connection = SqliteConnection::establish(location).context(ConnectSnafu { path })?;
    connection
        .batch_execute(CONNECTION_PRAGMAS)
        .context(DatabaseSnafu {
            operation: "configure connection",
        })?;
    Ok(connection)
}

pub fn initialize(path: &Path) -> Result<(), ChatError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).context(IoSnafu {
        operation: "create database directory",
        path: parent,
    })?;
    let mut connection = connect(path)?;
    connection
        .batch_execute("PRAGMA journal_mode = WAL;")
        .context(DatabaseSnafu {
            operation: "enable database WAL",
        })?;
    connection
        .run_pending_migrations(MIGRATIONS)
        .context(MigrationSnafu)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::prelude::*;

    #[test]
    fn migrations_round_trip_and_reopen_preserves_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("chat.sqlite");
        let database = ChatDatabase::open(&path).unwrap();
        let mut connection = connect(database.path()).unwrap();
        connection.revert_all_migrations(MIGRATIONS).unwrap();
        connection.run_pending_migrations(MIGRATIONS).unwrap();
        connection.batch_execute("INSERT INTO sessions (id, title, title_source, title_revision, profile, system, archived, next_sequence, created_at, updated_at) VALUES ('session', 'New conversation', 'provisional', 0, 'default', NULL, 0, 0, 1, 1);").unwrap();
        drop(connection);
        drop(database);
        let reopened = ChatDatabase::open(&path).unwrap();
        assert_eq!(
            crate::schema::sessions::table
                .count()
                .get_result::<i64>(&mut connect(reopened.path()).unwrap())
                .unwrap(),
            1
        );
    }

    #[test]
    fn upgrading_existing_records_preserves_usage_metadata_and_legacy_fingerprints() {
        use crate::schema::{model_calls, sessions};
        use std::collections::BTreeMap;

        for (previous_version, has_list, tokens) in
            [(1, false, 4), (2, true, 4), (3, false, 0), (3, true, 4)]
        {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("chat.sqlite");
            let database = ChatDatabase::open(&path).unwrap();
            let session = database
                .create_session("default", None, Some("Existing".into()))
                .unwrap();
            let begun = database
                .begin_turn(
                    &session.id,
                    "question",
                    crate::ModelIdentity {
                        profile: "default".into(),
                        model: Some("old-model".into()),
                    },
                    None,
                )
                .unwrap();
            let counters = BTreeMap::from([("input_tokens".into(), tokens)]);
            let metadata = (previous_version > 1).then(|| {
                BTreeMap::from([
                    ("source".into(), mf_runtime::ValueRef::from("retained")),
                    ("nullable".into(), mf_runtime::ValueRef::null()),
                    (
                        "nested".into(),
                        serde_json::json!({"value":null,"items":[null,true]}).into(),
                    ),
                ])
            });
            let reported = crate::CallUsage {
                timing: None,
                attribution: None,
                started_at: Some(1),
                call_id: "reported-call".into(),
                node: None,
                model: "old-model".into(),
                usage: Some(counters.clone()),
                error: None,
            };
            database
                .complete_turn(
                    begun.invocation.clone(),
                    crate::ConversationResult {
                        update: crate::MessageUpdate::Append {
                            messages: vec![mfn_llm::ChatMessage::Assistant {
                                content: "old reply".into(),
                                tool_calls: None,
                                metadata: metadata.clone(),
                            }],
                        },
                        usages: if has_list {
                            vec![reported.clone()]
                        } else {
                            Vec::new()
                        },
                        ..Default::default()
                    },
                )
                .unwrap();
            let original = database.call(&begun.invocation.call_id).unwrap();
            let mut connection = connect(&path).unwrap();
            diesel::update(model_calls::table.find(&begun.invocation.call_id))
                .set(model_calls::model.eq(Some("old-model")))
                .execute(&mut connection)
                .unwrap();
            if !has_list {
                diesel::update(model_calls::table.find(&begun.invocation.call_id))
                    .set(model_calls::usages.eq::<Option<String>>(None))
                    .execute(&mut connection)
                    .unwrap();
            }
            while connection.applied_migrations().unwrap().len() > previous_version {
                connection.revert_last_migration(MIGRATIONS).unwrap();
            }
            assert_eq!(
                sessions::table
                    .select(diesel::dsl::sql::<diesel::sql_types::BigInt>(
                        "conversation_revision"
                    ))
                    .get_result::<i64>(&mut connection)
                    .unwrap(),
                1
            );
            diesel::update(model_calls::table.find(&begun.invocation.call_id))
                .set(model_calls::input_tokens.eq(Some(tokens)))
                .execute(&mut connection)
                .unwrap();
            connection
                .batch_execute("UPDATE model_calls SET config_id = 'legacy-config';")
                .unwrap();
            drop(connection);
            drop(database);
            let reopened = ChatDatabase::open(path).unwrap();
            let call = reopened.call(&begun.invocation.call_id).unwrap();
            let expected = if has_list {
                reported
            } else {
                crate::CallUsage {
                    timing: None,
                    attribution: None,
                    started_at: original.started_at,
                    call_id: original.id.clone(),
                    node: None,
                    model: "old-model".into(),
                    usage: Some(counters),
                    error: None,
                }
            };
            assert_eq!(call.usages, Some(vec![expected]));
            assert_eq!(call.input_tokens, Some(tokens));
            assert_eq!(call.result_text.as_deref(), Some("old reply"));
            assert_eq!(call.metadata, metadata);
            assert!(matches!(
                &reopened.model_context(&session.id).unwrap().messages[1],
                mfn_llm::ChatMessage::Assistant { metadata: actual, .. } if actual == &metadata
            ));
            assert_eq!(
                model_calls::table
                    .find(&begun.invocation.call_id)
                    .select(diesel::dsl::sql::<diesel::sql_types::Text>("config_id"))
                    .get_result::<String>(&mut connect(reopened.path()).unwrap())
                    .unwrap(),
                "legacy-config"
            );
            let crate::contract::InvocationState::Committed(saved) =
                reopened.inspect_invocation(&begun.invocation).unwrap()
            else {
                panic!("expected migrated completion");
            };
            assert_eq!(
                reopened
                    .complete_turn(begun.invocation, saved.result.clone())
                    .unwrap(),
                saved
            );
            let usage = reopened.usage(Default::default()).unwrap();
            assert_eq!(usage.overall.attempted_calls, 1);
            assert_eq!(usage.overall.input_tokens.known_subtotal, Some(tokens));
            assert_eq!(usage.overall.output_tokens.known_subtotal, None);
            assert_eq!(usage.overall.performance.elapsed.reported_calls, 0);
            assert_eq!(usage.overall.performance.first_token.mean_ms, None);
            assert_eq!(usage.overall.performance.end_to_end.tokens_per_second, None);
        }
    }

    #[test]
    fn legacy_v6_exchanges_seed_context_and_terminal_results_exactly_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("chat.sqlite");
        let database = ChatDatabase::open(&path).unwrap();
        let session = database
            .create_session(
                "default",
                Some("Instructions".into()),
                Some("Existing".into()),
            )
            .unwrap();
        let first = database
            .begin_turn(
                &session.id,
                "first question",
                crate::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
            )
            .unwrap();
        database
            .complete_turn(
                first.invocation.clone(),
                crate::ConversationResult::assistant("first answer"),
            )
            .unwrap();
        let second = database
            .begin_turn(
                &session.id,
                "second question",
                crate::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
            )
            .unwrap();
        database
            .complete_turn(
                second.invocation.clone(),
                crate::ConversationResult::assistant("second answer"),
            )
            .unwrap();
        let original = database.messages(&session.id, 50, None).unwrap().messages;
        assert_eq!(original.len(), 4);
        let mut connection = connect(&path).unwrap();
        while connection.applied_migrations().unwrap().len() > 6 {
            connection.revert_last_migration(MIGRATIONS).unwrap();
        }
        drop(connection);
        drop(database);
        let reopened = ChatDatabase::open(&path).unwrap();
        let owner = reopened.session(&session.id).unwrap();
        assert_eq!(owner.context_revision, 2);
        let page = reopened.messages(&session.id, 50, None).unwrap().messages;
        assert_eq!(
            page.iter()
                .map(|record| &record.message)
                .collect::<Vec<_>>(),
            original
                .iter()
                .map(|record| &record.message)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            reopened
                .turn(&first.invocation.turn_id)
                .unwrap()
                .context_revision,
            0
        );
        assert_eq!(
            reopened
                .turn(&second.invocation.turn_id)
                .unwrap()
                .context_revision,
            1
        );
        let context = reopened.model_context(&session.id).unwrap();
        assert_eq!(context.revision, 2);
        assert_eq!(context.messages.len(), 5);
        assert!(
            matches!(&context.messages[0], mfn_llm::ChatMessage::System { content, .. } if content == "Instructions")
        );
        assert!(
            matches!(&context.messages[4], mfn_llm::ChatMessage::Assistant { content, .. } if content == "second answer")
        );
        assert!(
            reopened
                .call(&first.invocation.call_id)
                .unwrap()
                .terminal_result
                .is_some()
        );
        assert!(
            reopened
                .call(&second.invocation.call_id)
                .unwrap()
                .terminal_result
                .is_some()
        );
        assert_eq!(reopened.model_context(&session.id).unwrap(), context);
    }

    #[test]
    fn ownership_precedes_initialization_and_is_released_on_failure() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("chat.sqlite");
        let database = ChatDatabase::open(&path).unwrap();
        assert!(matches!(
            ChatDatabase::open(&path),
            Err(ChatError::Busy { .. })
        ));
        drop(database);
        ChatDatabase::open(&path).unwrap();

        let invalid = directory.path().join("directory");
        fs::create_dir(&invalid).unwrap();
        assert!(matches!(
            ChatDatabase::open(&invalid),
            Err(ChatError::Connect { .. })
        ));
        assert!(matches!(
            ChatDatabase::open(&invalid),
            Err(ChatError::Connect { .. })
        ));
    }
}
