use crate::{
    ChatError,
    error::*,
    nodes::{Initialize, InitializeInputs},
};
use diesel::{Connection, connection::SimpleConnection, sqlite::SqliteConnection};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use fs2::FileExt;
use mf_runtime::{ExecutionContext, TypedTaskNode};
use snafu::{IntoError, OptionExt, ResultExt};
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");
const CONNECTION_PRAGMAS: &str = "PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;";

/// Owns initialization and recovery rights for one database until dropped.
pub struct ChatDatabase {
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
        let database_path = path.to_str().context(PathSnafu { path: &path })?.to_owned();
        Initialize.execute(
            InitializeInputs { database_path },
            &mut ExecutionContext::default(),
        )?;
        let database = Self {
            path,
            _ownership: ownership,
        };
        let _: crate::nodes::RecoveryOutputs = crate::operation::execute(
            database.path(),
            "mf_chat.storage.recover",
            crate::nodes::RecoveryInputs {
                database_path: String::new(),
            },
        )?;
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
        connection.batch_execute("INSERT INTO sessions VALUES ('session', 'New conversation', 'provisional', 0, 0, 'default', NULL, 0, 0, 1, 1);").unwrap();
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
            Err(ChatError::Node { .. })
        ));
        assert!(matches!(
            ChatDatabase::open(&invalid),
            Err(ChatError::Node { .. })
        ));
    }
}
