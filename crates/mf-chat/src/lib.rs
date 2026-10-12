//! Persistent chat operations implemented as application-owned workflow nodes.

mod error;
mod models;
pub mod nodes;
mod schema;
mod storage;

pub use error::ChatError;
pub use models::{Message, ModelCall, Session, Turn};
pub use storage::ChatDatabase;
