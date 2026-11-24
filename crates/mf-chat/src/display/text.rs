//! Reloadable transcript bodies. Temporary data is never part of the session database.

use super::Reader;
use crate::{ChatError, error::*};
use diesel::{connection::SimpleConnection, prelude::*, sql_types};
use snafu::{OptionExt, ResultExt, ensure};
use std::{
    fmt,
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, Default)]
pub struct Text(Option<Arc<Source>>);

// A handle is immutable, including the end of a streamed prefix. Cloning a handle
// neither reads the body nor keeps the rendered document alive.
impl PartialEq for Text {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

#[derive(Debug)]
enum Source {
    Message {
        reader: Reader,
        id: Arc<str>,
        argument: Option<usize>,
    },
    Fragment {
        store: Fragments,
        id: i64,
        bytes: usize,
    },
    Inline(String),
}

#[derive(diesel::QueryableByName)]
struct StringRow {
    #[diesel(sql_type = sql_types::Text)]
    value: String,
}

impl Text {
    pub(super) fn saved(reader: &Reader, id: &Arc<str>, argument: Option<usize>) -> Self {
        Self(Some(Arc::new(Source::Message {
            reader: reader.clone(),
            id: id.clone(),
            argument,
        })))
    }

    pub(super) fn inline(text: String) -> Self {
        if text.is_empty() {
            Self::default()
        } else {
            Self(Some(Arc::new(Source::Inline(text))))
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    pub fn load(&self) -> Result<String, ChatError> {
        match self.0.as_deref() {
            None => Ok(String::new()),
            Some(Source::Inline(text)) => Ok(text.clone()),
            Some(Source::Fragment { store, id, bytes }) => store.read(*id, *bytes),
            Some(Source::Message {
                reader,
                id,
                argument,
            }) => {
                let conn = &mut *reader.0.lock().unwrap();
                if let Some(index) = argument {
                    // The JSON operator retains JSON strings, booleans and null as JSON.
                    let row = diesel::sql_query(
                        "SELECT payload -> ? AS value FROM messages WHERE id = ?",
                    )
                    .bind::<sql_types::Text, _>(format!("$.tool_calls[{index}].arguments"))
                    .bind::<sql_types::Text, _>(&**id)
                    .get_result::<StringRow>(conn)
                    .context(DatabaseSnafu {
                        operation: "read saved tool arguments",
                    })?;
                    let value: mf_runtime::ValueRef =
                        serde_json::from_str(&row.value).context(JsonSnafu)?;
                    serde_json::to_string_pretty(&value).context(JsonSnafu)
                } else {
                    Ok(diesel::sql_query(
                        "SELECT payload ->> '$.content' AS value FROM messages WHERE id = ?",
                    )
                    .bind::<sql_types::Text, _>(&**id)
                    .get_result::<StringRow>(conn)
                    .context(DatabaseSnafu {
                        operation: "read saved message content",
                    })?
                    .value)
                }
            }
        }
    }
}

/// A private, file-backed SQLite database. SQLite deletes it on close; the last
/// snapshot owns its lifetime, not the controller or the durable chat session.
#[derive(Clone)]
pub struct Fragments(Arc<Mutex<SqliteConnection>>);

impl fmt::Debug for Fragments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Fragments")
    }
}

impl Fragments {
    pub fn open() -> Result<Self, ChatError> {
        // An empty SQLite filename creates an automatically deleted temporary database,
        // unlike :memory:. Bound its pager even for very long unfinished turns.
        let mut conn = SqliteConnection::establish("").context(ConnectSnafu {
            path: Path::new("<temporary transcript>"),
        })?;
        conn.batch_execute(
            "PRAGMA cache_size = -512; PRAGMA temp_store = FILE; PRAGMA synchronous = OFF;
            CREATE TABLE fragments (id INTEGER PRIMARY KEY, parent INTEGER, value TEXT NOT NULL);",
        )
        .context(DatabaseSnafu {
            operation: "initialize temporary transcript",
        })?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    pub fn append(&self, prefix: &Text, fragment: &str) -> Result<Text, ChatError> {
        if fragment.is_empty() {
            return Ok(prefix.clone());
        }
        let (parent, size) = match prefix.0.as_deref() {
            None => (None, 0),
            Some(Source::Fragment { store, id, bytes }) if Arc::ptr_eq(&self.0, &store.0) => {
                (Some(*id), *bytes)
            }
            _ => {
                return InvalidSnafu {
                    field: "display fragment",
                    reason: "prefix belongs to another source",
                }
                .fail();
            }
        };
        let bytes = size.checked_add(fragment.len()).context(ArithmeticSnafu {
            operation: "size temporary transcript",
        })?;
        #[derive(diesel::QueryableByName)]
        struct Row {
            #[diesel(sql_type = sql_types::BigInt)]
            id: i64,
        }
        let id =
            diesel::sql_query("INSERT INTO fragments (parent, value) VALUES (?, ?) RETURNING id")
                .bind::<sql_types::Nullable<sql_types::BigInt>, _>(parent)
                .bind::<sql_types::Text, _>(fragment)
                .get_result::<Row>(&mut *self.0.lock().unwrap())
                .context(DatabaseSnafu {
                    operation: "append temporary transcript fragment",
                })?
                .id;
        Ok(Text(Some(Arc::new(Source::Fragment {
            store: self.clone(),
            id,
            bytes,
        }))))
    }

    fn read(&self, id: i64, bytes: usize) -> Result<String, ChatError> {
        // Order only the row IDs; do not build an aggregate SQL string or a Vec of all fragments.
        let mut conn = self.0.lock().unwrap();
        let rows = diesel::sql_query("WITH RECURSIVE chain(id) AS (
                SELECT ? UNION ALL SELECT f.parent FROM fragments f JOIN chain ON f.id = chain.id WHERE f.parent IS NOT NULL
            ) SELECT value FROM fragments WHERE id IN chain ORDER BY id")
            .bind::<sql_types::BigInt, _>(id).load_iter::<StringRow, diesel::connection::DefaultLoadingMode>(&mut *conn)
            .context(DatabaseSnafu { operation: "read temporary transcript" })?;
        let mut result = String::with_capacity(bytes);
        for row in rows {
            result.push_str(
                &row.context(DatabaseSnafu {
                    operation: "read temporary transcript fragment",
                })?
                .value,
            );
        }
        ensure!(
            result.len() == bytes,
            InvalidSnafu {
                field: "temporary transcript",
                reason: "incomplete fragment chain"
            }
        );
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_scratch_append_keeps_earlier_handles_and_later_messages_readable() {
        use super::super::{Message, stage_with};
        let store = Fragments::open().unwrap();
        store
            .0
            .lock()
            .unwrap()
            .batch_execute(
                "CREATE TRIGGER fail_append BEFORE INSERT ON fragments WHEN NEW.value = 'question'
             BEGIN SELECT RAISE(ABORT, 'scratch full'); END;",
            )
            .unwrap();
        let messages = serde_json::from_value(serde_json::json!([
            {"role":"system", "content":"instruction"},
            {"role":"user", "content":"question"},
            {"role":"assistant", "content":"", "tool_calls":[{"id":"id", "name":"lookup", "arguments":{"q":"query"}}]},
            {"role":"tool", "tool_call_id":"id", "content":"result"}
        ])).unwrap();
        let (staged, error) = stage_with(messages, Ok(store));
        assert!(matches!(
            error,
            Some(ChatError::Database {
                operation: "append temporary transcript fragment",
                ..
            })
        ));
        let bodies: Vec<_> = staged.iter().map(|m| m.content().load().unwrap()).collect();
        assert_eq!(bodies, ["instruction", "question", "", "result"]);
        let Message::Assistant { tools, .. } = &staged[2] else {
            panic!("expected assistant")
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&tools[0].arguments.load().unwrap()).unwrap(),
            serde_json::json!({"q":"query"})
        );
    }

    #[test]
    fn an_incomplete_fragment_chain_never_returns_a_partial_body() {
        let store = Fragments::open().unwrap();
        let head = store.append(&Text::default(), "one").unwrap();
        let tail = store.append(&head, "two").unwrap();
        store
            .0
            .lock()
            .unwrap()
            .batch_execute("DELETE FROM fragments WHERE parent IS NULL")
            .unwrap();
        assert!(matches!(
            tail.load(),
            Err(ChatError::Invalid {
                field: "temporary transcript",
                reason: "incomplete fragment chain"
            })
        ));
    }

    #[test]
    fn snapshots_read_exact_branches_and_release_the_database_with_the_last_handle() {
        let db = Fragments::open().unwrap();
        let lifetime = Arc::downgrade(&db.0);
        let chunk = "é\0x".repeat(256);
        let mut prefix = Text::default();
        for _ in 0..1500 {
            prefix = db.append(&prefix, &chunk).unwrap();
        }
        let left = db.append(&prefix, "-left").unwrap();
        let right = db.append(&prefix, "-right").unwrap();
        assert!(
            Fragments::open()
                .unwrap()
                .append(&prefix, "foreign")
                .is_err()
        );
        drop(db);
        assert_eq!(prefix.load().unwrap(), chunk.repeat(1500));
        assert_eq!(left.load().unwrap(), format!("{}-left", chunk.repeat(1500)));
        drop((prefix, left));
        assert!(right.load().unwrap().ends_with("-right"));
        drop(right);
        assert!(lifetime.upgrade().is_none());
    }
}
