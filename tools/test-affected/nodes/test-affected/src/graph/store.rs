use crate::{
    TestCase,
    graph::{
        CoverageManifest, lcov,
        schema::{coverage, files, records, tests as graph_tests},
    },
};
use diesel::{
    connection::SimpleConnection,
    expression::BoxableExpression,
    prelude::*,
    sql_types::Bool,
    sqlite::{Sqlite, SqliteConnection},
};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error as StdError,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, SystemTimeError, UNIX_EPOCH},
};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("coverage graph {path:?} does not exist"))]
    Missing { path: PathBuf },
    #[snafu(display("coverage graph path {path:?} must be UTF-8"))]
    Path { path: PathBuf },
    #[snafu(display("could not connect to coverage graph {path:?}: {source}"))]
    Connect {
        path: PathBuf,
        source: diesel::ConnectionError,
    },
    #[snafu(display("could not migrate coverage graph {path:?}: {source}"))]
    Migration {
        path: PathBuf,
        source: Box<dyn StdError + Send + Sync>,
    },
    #[snafu(display("could not {operation}: {source}"))]
    Database {
        operation: &'static str,
        source: diesel::result::Error,
    },
    #[snafu(transparent)]
    Query { source: diesel::result::Error },
    #[snafu(display("coverage graph already indexes commit {commit_sha}"))]
    DuplicateCommit { commit_sha: String },
    #[snafu(display("invalid coverage manifest {field}: {reason}"))]
    InvalidManifest {
        field: &'static str,
        reason: &'static str,
    },
    #[snafu(display("could not {action} at {path:?}: {source}"))]
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("could not {action}: {source}"))]
    Json {
        action: &'static str,
        source: serde_json::Error,
    },
    #[snafu(display("invalid lcov export {path:?}: malformed {record} record"))]
    Lcov { path: PathBuf, record: &'static str },
    #[snafu(display("invalid lcov export {path:?}: non-numeric {record} record: {source}"))]
    LcovNumber {
        path: PathBuf,
        record: &'static str,
        source: std::num::ParseIntError,
    },
    #[snafu(display("invalid coverage query range {start}..={end}"))]
    InvalidRange { start: u32, end: u32 },
    #[snafu(display("could not read UTC time: {source}"))]
    Clock { source: SystemTimeError },
    #[snafu(display("UTC timestamp is out of range: {source}"))]
    Timestamp { source: std::num::TryFromIntError },
}

/// Schema version recorded in every commit record. Readers compare it
/// against their own expectation before consuming a graph.
pub const SCHEMA_VERSION: i64 = 1;

/// Inclusive line interval within one workspace source file.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(deny_unknown_fields)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}
const CONNECTION_PRAGMAS: &str = "PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;";
const READER_PRAGMAS: &str =
    "PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON; PRAGMA query_only = ON;";
// Multi-row inserts stay below the conservative SQLite bind-parameter limit.
const COVERAGE_INSERT_CHUNK: usize = 150;

fn connect(path: &Path, pragmas: &str) -> Result<SqliteConnection, Error> {
    let location = path.to_str().context(PathSnafu { path })?;
    let mut connection = SqliteConnection::establish(location).context(ConnectSnafu { path })?;
    connection.batch_execute(pragmas).context(DatabaseSnafu {
        operation: "configure coverage graph connection",
    })?;
    Ok(connection)
}

/// One indexed commit: graph identity, producing run, schema version, and
/// execution signature for compatibility decisions by the caller.
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Queryable, Selectable, Insertable,
)]
#[diesel(table_name = records)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct Record {
    pub commit_sha: String,
    pub run_id: String,
    pub attempt: i64,
    pub schema_version: i64,
    pub signature: String,
    pub created_at: i64,
}

/// Summary of one indexed commit, for collection-side reporting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedCommit {
    pub commit_sha: String,
    pub tests: usize,
    pub files: usize,
    pub ranges: usize,
}

#[derive(Insertable)]
#[diesel(table_name = graph_tests)]
struct NewTest<'a> {
    package: &'a str,
    binary: &'a str,
    name: &'a str,
}

#[derive(Insertable)]
#[diesel(table_name = files)]
struct NewFile<'a> {
    path: &'a str,
}

#[derive(Insertable)]
#[diesel(table_name = coverage)]
struct NewCoverage {
    commit_sha: String,
    file_id: i64,
    line_start: i64,
    line_end: i64,
    test_id: i64,
}

fn overlaps(
    range: LineRange,
) -> Box<dyn BoxableExpression<coverage::table, Sqlite, SqlType = Bool>> {
    Box::new(
        coverage::line_start
            .le(i64::from(range.end))
            .and(coverage::line_end.ge(i64::from(range.start))),
    )
}

/// Writes coverage graphs. Each commit is indexed once, transactionally; the
/// resulting file is immutable and published as-is.
pub struct GraphWriter {
    connection: SqliteConnection,
}

impl GraphWriter {
    pub fn create(path: &Path) -> Result<Self, Error> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).context(IoSnafu {
                action: "create coverage graph directory",
                path: parent,
            })?;
        }
        let mut connection = connect(path, CONNECTION_PRAGMAS)?;
        connection
            .run_pending_migrations(MIGRATIONS)
            .context(MigrationSnafu { path })?;
        Ok(Self { connection })
    }

    /// Indexes one commit from its per-test coverage export manifest.
    ///
    /// All manifest lcov files are parsed before the single write transaction
    /// starts; any error leaves the graph untouched.
    pub fn index(
        &mut self,
        manifest: &CoverageManifest,
        workspace_root: &Path,
    ) -> Result<IndexedCommit, Error> {
        ensure!(
            !manifest.commit_sha.is_empty(),
            InvalidManifestSnafu {
                field: "commit_sha",
                reason: "must not be empty"
            }
        );
        let mut identities = BTreeSet::new();
        let mut parsed = Vec::with_capacity(manifest.tests.len());
        for test in &manifest.tests {
            ensure!(
                !(test.package.is_empty() || test.binary.is_empty() || test.name.is_empty()),
                InvalidManifestSnafu {
                    field: "tests",
                    reason: "package, binary, and name must not be empty"
                }
            );
            ensure!(
                identities.insert((&test.package, &test.binary, &test.name)),
                InvalidManifestSnafu {
                    field: "tests",
                    reason: "test identities must be unique"
                }
            );
            parsed.push((test, lcov::parse(&test.lcov, workspace_root)?));
        }
        let created_at = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context(ClockSnafu)?
                .as_secs(),
        )
        .context(TimestampSnafu)?;
        self.connection.transaction(|connection| {
            let indexed: i64 = records::table
                .filter(records::commit_sha.eq(&manifest.commit_sha))
                .count()
                .get_result(connection)
                .context(DatabaseSnafu {
                    operation: "read coverage graph records",
                })?;
            ensure!(
                indexed == 0,
                DuplicateCommitSnafu {
                    commit_sha: manifest.commit_sha.clone()
                }
            );
            diesel::insert_into(records::table)
                .values(Record {
                    commit_sha: manifest.commit_sha.clone(),
                    run_id: manifest.run_id.clone(),
                    attempt: i64::from(manifest.attempt),
                    schema_version: SCHEMA_VERSION,
                    signature: manifest.signature.clone(),
                    created_at,
                })
                .execute(connection)
                .context(DatabaseSnafu {
                    operation: "record coverage graph commit",
                })?;
            let mut test_ids: BTreeMap<(&str, &str, &str), i64> = BTreeMap::new();
            let mut file_ids: BTreeMap<&str, i64> = BTreeMap::new();
            let mut rows = Vec::new();
            for (test, hit_files) in &parsed {
                let identity = (
                    test.package.as_str(),
                    test.binary.as_str(),
                    test.name.as_str(),
                );
                let test_id = match test_ids.get(&identity) {
                    Some(id) => *id,
                    None => {
                        diesel::insert_into(graph_tests::table)
                            .values(NewTest {
                                package: &test.package,
                                binary: &test.binary,
                                name: &test.name,
                            })
                            .on_conflict_do_nothing()
                            .execute(connection)
                            .context(DatabaseSnafu {
                                operation: "record coverage graph test",
                            })?;
                        let id = graph_tests::table
                            .filter(graph_tests::package.eq(&test.package))
                            .filter(graph_tests::binary.eq(&test.binary))
                            .filter(graph_tests::name.eq(&test.name))
                            .select(graph_tests::id)
                            .first::<i64>(connection)
                            .context(DatabaseSnafu {
                                operation: "read coverage graph test",
                            })?;
                        test_ids.insert(identity, id);
                        id
                    }
                };
                for (file, ranges) in hit_files {
                    let file_id = match file_ids.get(file.as_str()) {
                        Some(id) => *id,
                        None => {
                            diesel::insert_into(files::table)
                                .values(NewFile { path: file })
                                .on_conflict_do_nothing()
                                .execute(connection)
                                .context(DatabaseSnafu {
                                    operation: "record coverage graph file",
                                })?;
                            let id = files::table
                                .filter(files::path.eq(file))
                                .select(files::id)
                                .first::<i64>(connection)
                                .context(DatabaseSnafu {
                                    operation: "read coverage graph file",
                                })?;
                            file_ids.insert(file, id);
                            id
                        }
                    };
                    rows.extend(ranges.iter().map(|range| NewCoverage {
                        commit_sha: manifest.commit_sha.clone(),
                        file_id,
                        line_start: i64::from(range.start),
                        line_end: i64::from(range.end),
                        test_id,
                    }));
                }
            }
            for chunk in rows.chunks(COVERAGE_INSERT_CHUNK) {
                diesel::insert_into(coverage::table)
                    .values(chunk)
                    .execute(connection)
                    .context(DatabaseSnafu {
                        operation: "record coverage intervals",
                    })?;
            }
            Ok(IndexedCommit {
                commit_sha: manifest.commit_sha.clone(),
                tests: parsed.len(),
                files: file_ids.len(),
                ranges: rows.len(),
            })
        })
    }
}

/// Query-only coverage graph reader. Compatibility (schema version and
/// execution signature) is the caller's decision; the reader exposes the
/// stored values through [`GraphReader::record`].
pub struct GraphReader {
    connection: SqliteConnection,
}

impl GraphReader {
    pub fn open(path: &Path) -> Result<Self, Error> {
        ensure!(path.is_file(), MissingSnafu { path });
        let connection = connect(path, READER_PRAGMAS)?;
        Ok(Self { connection })
    }

    /// Returns the indexed record for `commit_sha`, if present.
    pub fn record(&mut self, commit_sha: &str) -> Result<Option<Record>, Error> {
        records::table
            .find(commit_sha)
            .select(Record::as_select())
            .first(&mut self.connection)
            .optional()
            .context(DatabaseSnafu {
                operation: "read coverage graph record",
            })
    }

    /// Returns whether the graph holds any coverage rows for `path` at
    /// `commit_sha`. Distinguishes "the graph does not know this file" from
    /// "the changed ranges have no covering test".
    pub fn has_file(&mut self, commit_sha: &str, path: &str) -> Result<bool, Error> {
        let rows = coverage::table
            .filter(coverage::commit_sha.eq(commit_sha))
            .filter(
                coverage::file_id
                    .eq_any(files::table.filter(files::path.eq(path)).select(files::id)),
            )
            .count()
            .get_result::<i64>(&mut self.connection)
            .context(DatabaseSnafu {
                operation: "check coverage graph file",
            })?;
        Ok(rows > 0)
    }

    /// Maps changed inclusive line ranges in one workspace-relative file to
    /// the distinct tests covering them at `commit_sha`, in one indexed
    /// query. Unknown commits, files, and uncovered ranges yield no rows.
    pub fn tests_covering(
        &mut self,
        commit_sha: &str,
        path: &str,
        ranges: &[LineRange],
    ) -> Result<Vec<TestCase>, Error> {
        for range in ranges {
            ensure!(
                range.start >= 1 && range.start <= range.end,
                InvalidRangeSnafu {
                    start: range.start,
                    end: range.end
                }
            );
        }
        if ranges.is_empty() {
            return Ok(Vec::new());
        }
        // OR the overlap alternatives first so the commit and file filters
        // still apply to every alternative once ANDed in below.
        let mut covering = coverage::table.into_boxed();
        for &range in ranges {
            covering = covering.or_filter(overlaps(range));
        }
        let covering = covering
            .filter(coverage::commit_sha.eq(commit_sha))
            .filter(
                coverage::file_id
                    .eq_any(files::table.filter(files::path.eq(path)).select(files::id)),
            )
            .select(coverage::test_id)
            .distinct();
        let rows = graph_tests::table
            .filter(graph_tests::id.eq_any(covering))
            .select((graph_tests::package, graph_tests::binary, graph_tests::name))
            .load::<(String, String, String)>(&mut self.connection)
            .context(DatabaseSnafu {
                operation: "query covering tests",
            })?;
        Ok(rows
            .into_iter()
            .map(|(package, binary, name)| TestCase {
                package,
                binary,
                name,
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::QueryableByName;

    #[derive(QueryableByName)]
    struct QueryOnly {
        #[diesel(sql_type = diesel::sql_types::Integer)]
        query_only: i32,
    }

    #[test]
    fn reader_connections_are_query_only_and_reject_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("graph.sqlite");
        GraphWriter::create(&path).unwrap();
        let mut reader = GraphReader::open(&path).unwrap();
        let mode = diesel::sql_query("PRAGMA query_only")
            .get_result::<QueryOnly>(&mut reader.connection)
            .unwrap();
        assert_eq!(mode.query_only, 1);
        assert!(
            reader
                .connection
                .batch_execute("INSERT INTO records VALUES ('c', 'r', 1, 1, 's', 0)")
                .is_err()
        );
        assert!(reader.record("c").unwrap().is_none());
    }

    #[test]
    fn reader_rejects_missing_graphs() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("absent.sqlite");
        assert!(matches!(
            GraphReader::open(&path),
            Err(Error::Missing { .. })
        ));
        assert!(!path.exists());
    }
}
