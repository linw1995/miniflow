use crate::schema::{messages, model_calls, sessions, turns};
use diesel::{Insertable, Queryable, Selectable};
use mf_runtime::NodeValue;
use serde::{Deserialize, Serialize};

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    NodeValue,
    Queryable,
    Selectable,
    Insertable,
)]
#[diesel(table_name = sessions)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct Session {
    pub id: String,
    pub title: String,
    pub title_source: String,
    pub title_revision: i64,
    pub profile: String,
    pub system: Option<String>,
    pub archived: bool,
    pub next_sequence: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    NodeValue,
    Queryable,
    Selectable,
    Insertable,
)]
#[diesel(table_name = turns)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct Turn {
    pub id: String,
    pub session_id: String,
    pub retry_of: Option<String>,
    pub sequence: i64,
    pub state: String,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    NodeValue,
    Queryable,
    Selectable,
    Insertable,
)]
#[diesel(table_name = messages)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub turn_id: String,
    pub sequence: i64,
    pub role: String,
    pub content: String,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[diesel(deserialize_as = crate::storage::StoredJson<mfn_llm::MessageMetadata>)]
    #[diesel(serialize_as = crate::storage::StoredJson<mfn_llm::MessageMetadata>)]
    pub metadata: Option<mfn_llm::MessageMetadata>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    NodeValue,
    Queryable,
    Selectable,
    Insertable,
)]
#[diesel(table_name = model_calls)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct ModelCall {
    pub id: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub purpose: String,
    pub profile: String,
    pub model: Option<String>,
    pub state: String,
    pub title_revision: Option<i64>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    /// Legacy scalar counters; new replies persist call facts in usages.
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub finish_reason: Option<String>,
    pub error: Option<String>,
    pub result_text: Option<String>,
    pub disposition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[diesel(deserialize_as = crate::storage::StoredJson<Vec<crate::CallUsage>>)]
    #[diesel(serialize_as = crate::storage::StoredJson<Vec<crate::CallUsage>>)]
    pub usages: Option<Vec<crate::CallUsage>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[diesel(deserialize_as = crate::storage::StoredJson<mfn_llm::MessageMetadata>)]
    #[diesel(serialize_as = crate::storage::StoredJson<mfn_llm::MessageMetadata>)]
    pub metadata: Option<mfn_llm::MessageMetadata>,
    pub result_title: Option<String>,
}
