//! Persistent chat operations implemented as application-owned workflow nodes.

mod error;
pub mod flows;
mod host;
mod model;
mod models;
#[macro_use]
pub mod nodes;
mod operation;
mod schema;
mod session;
mod storage;
mod title;
mod turn;
mod usage;

pub use error::ChatError;
pub use host::{ChatHost, ChatJob, HostConfig, ModelProfile, TitleJob, UncommittedCompletion};
pub use models::{Message, ModelCall, Session, Turn};
pub use session::{DEFAULT_HISTORY_BYTES, HistoryContext, MessagePage, MessageRecord, SessionPage};
pub use storage::ChatDatabase;
pub use title::{CallPurpose, TitleCompletion, TitleOutcome};
pub use turn::{Completion, Invocation, ModelIdentity, Reply};
pub use usage::{
    TokenCounter, UsageGroup, UsageGrouping, UsageQuery, UsageStatistics, UsageSummary,
};
