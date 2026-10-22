//! Immutable per-commit test coverage graphs.
//!
//! A coverage graph is one SQLite file mapping workspace source line
//! intervals to the distinct tests that covered them at one or more indexed
//! commits. Each commit binds its identity to the producing run, the graph
//! schema version, and the execution signature of the producing coverage
//! run. Writes are transactional and write-once per commit: indexing a
//! commit that already exists fails. Readers open the file query-only.
//!
//! The execution `signature` is normalized JSON text describing the
//! producing environment, aligned with the coverage identity recorded by
//! `scripts/run-cov.sh`: the platform-independent toolchain identity
//! (`rustc -vV` release and commit hash; line-to-test mapping does not depend
//! on the host architecture), the cargo feature set, the build target, the
//! nextest profile, and the coverage-relevant environment (`RUSTFLAGS`,
//! `CARGO_ENCODED_RUSTFLAGS`, `CARGO_INCREMENTAL`, `CARGO_PROFILE_DEV_DEBUG`,
//! `CARGO_PROFILE_TEST_DEBUG`). The store persists and exposes it; deciding
//! whether a graph is compatible with the current execution is the caller's
//! responsibility.
//!
//! Indexing consumes a per-test coverage export manifest; see
//! [`CoverageManifest`] for the JSON format. Each listed lcov export is
//! parsed for hit lines (`DA` records with a positive count) belonging to
//! workspace source files; consecutive hits compress into inclusive
//! [`LineRange`] intervals.

mod lcov;
mod manifest;
mod schema;
mod store;

pub use manifest::{CoverageManifest, CoveredTest};
pub use store::{
    Error, GraphReader, GraphWriter, IndexedCommit, LineRange, Record, SCHEMA_VERSION,
};
