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
mod tool_uses;
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
pub use tool_uses::{
    MAX_TOOL_NAME_BYTES, MAX_TOOL_TEXT_BYTES, MAX_TOOL_USES, ToolUse, ToolUseHistory, ToolUsePhase,
};
pub use turn::{Invocation, ModelIdentity};
pub use usage::CallPurpose;
pub use usage::{
    CallTiming, CallTokenRates, LatencySummary, PerformanceSummary, TokenCounter, TokenRateSummary,
    UsageGroup, UsageGrouping, UsageQuery, UsageStatistics, UsageSummary,
};

pub use mfn_llm::{MessageMetadata, TextDelta, UsageAttribution, UsageNode, UsageScope};

pub use workflow::{ConversationBackend, WorkflowProfile};
