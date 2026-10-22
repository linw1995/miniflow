//! Persistent chat operations shared by direct APIs and application-owned workflow nodes.

mod error;
pub mod flows;
mod host;
mod models;
#[macro_use]
pub mod nodes;
pub mod contract;
mod schema;
mod session;
mod storage;
mod title;
mod turn;
mod usage;

pub use contract::{
    Completion, ConversationContext, ConversationInputs, ConversationResult, TitleContext,
    UncommittedCompletion,
};
pub use error::ChatError;
pub use host::{ChatHost, ChatJob, HostConfig, ModelProfile};
pub use models::{Message, ModelCall, Session, Turn};
pub use session::{DEFAULT_HISTORY_BYTES, HistoryContext, MessagePage, MessageRecord, SessionPage};
pub use storage::ChatDatabase;
pub use turn::{Invocation, ModelIdentity};
pub use usage::CallPurpose;
pub use usage::{
    TokenCounter, UsageGroup, UsageGrouping, UsageQuery, UsageStatistics, UsageSummary,
};

pub use mfn_llm::{CallUsage, MessageMetadata, UsageAttribution, UsageNode, UsageScope};
