//! Persistent chat operations shared by direct APIs and application-owned workflow nodes.

mod error;
pub mod flows;
mod host;
mod models;
mod preview;
#[macro_use]
pub mod nodes;
mod call_events;
pub mod contract;
mod schema;
mod session;
mod storage;
mod title;
mod turn;
mod usage;
pub use call_events::{CallEventCollector, CallEventError};
pub mod workflow;

pub use contract::{
    CallUsage, Completion, ConversationContext, ConversationInputs, ConversationResult,
    TitleContext, UncommittedCompletion,
};
pub use error::{BackendSnafu, CaptureSnafu, ChatError};
pub use host::{ChatHost, ChatJob, HostConfig};
pub use models::{Message, ModelCall, Session, Turn};
pub use preview::{PreviewConsumer, PreviewEvent, PreviewReceiver};
pub use session::{HistoryContext, MessagePage, MessageRecord, SessionPage};
pub use storage::ChatDatabase;
pub use turn::{Invocation, ModelIdentity};
pub use usage::CallPurpose;
pub use usage::{
    CallTiming, CallTokenRates, LatencySummary, PerformanceSummary, TokenCounter, TokenRateSummary,
    UsageGroup, UsageGrouping, UsageQuery, UsageStatistics, UsageSummary,
};

pub use mfn_llm::{MessageMetadata, TextDelta, UsageAttribution, UsageNode, UsageScope};

pub use workflow::{ConversationBackend, WorkflowProfile};
