//! Persistent chat operations shared by direct APIs and application-owned workflow nodes.

mod error;
pub mod flows;
mod host;
mod model_selection;
mod models;
mod preview;
#[macro_use]
pub mod nodes;
mod call_events;
mod context;
pub mod contract;
mod schema;
mod session;
mod storage;
mod title;
mod tool_metrics;
mod turn;
mod usage;
pub use call_events::{CallEventCollector, CallEventError};
pub mod workflow;

pub use context::{ModelContext, context_tokens};
pub use contract::{
    CallUsage, CompactionConfig, CompactionContext, Completion, ConversationContext,
    ConversationInputs, ConversationResult, DEFAULT_COMPACTION_CONTEXT_WINDOW, MessageUpdate,
    ModelContextRevision, ResultAlternatives, TitleContext, UncommittedCompletion,
};
pub use error::{BackendSnafu, CaptureSnafu, ChatError};
pub use host::{ChatHost, ChatJob, HostConfig};
pub use model_selection::{ModelSelection, ModelSelectionCancellation, ModelSelectionUpdate};
pub use models::{Message, ModelCall, Session, Turn};
pub use preview::{PreviewConsumer, PreviewDelta, PreviewEvent, PreviewReceiver};
pub use session::{MessagePage, MessageRecord, SessionPage};
pub use storage::ChatDatabase;
pub use tool_metrics::{ToolExecution, ToolExecutionMetrics, ToolExecutionPhase};
pub use turn::{Invocation, ModelIdentity};
pub use usage::CallPurpose;
pub use usage::{
    CallTiming, CallTokenRates, LatencySummary, PerformanceSummary, TokenCounter, TokenRateSummary,
    UsageGroup, UsageGrouping, UsageQuery, UsageStatistics, UsageSummary,
};

pub use mfn_llm::{
    ChatMessage, MessageMetadata, ModelInfo, TextDelta, ToolCallDelta, UsageAttribution, UsageNode,
    UsageScope,
};

pub use workflow::{ConversationBackend, ModelContextWindowCache, WorkflowProfile};
