//! CLI-only home for workflow telemetry reception and terminal presentation.
//!
//! Protocol types live in `mf-telemetry`. Runtime and compiler crates must not
//! depend on this package. Reception, state reduction, and rendering are added
//! separately from the observation contract.

#[cfg(any(unix, windows))]
pub mod description;
mod duration;
pub mod graph;
#[cfg(unix)]
pub mod manifest;
pub mod receiver;
#[cfg(unix)]
pub mod run;
pub mod state;

mod snapshots;
pub use snapshots::HistorySnapshot;

#[cfg(all(test, unix))]
#[path = "../tests/common/mod.rs"]
mod test_support;
