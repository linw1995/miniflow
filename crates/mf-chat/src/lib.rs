//! Persistent chat operations implemented as application-owned workflow nodes.

mod error;
mod models;
#[macro_use]
pub mod nodes;
mod operation;
mod schema;
mod session;
mod storage;
mod turn;

pub use error::ChatError;
pub use models::{Message, ModelCall, Session, Turn};
pub use session::{DEFAULT_HISTORY_BYTES, HistoryContext, MessagePage, MessageRecord, SessionPage};
pub use storage::ChatDatabase;
pub use turn::{Completion, Invocation, ModelIdentity, Reply};
