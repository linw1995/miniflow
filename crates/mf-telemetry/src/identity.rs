use crate::{ContractError, require};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use snafu::ResultExt;
use std::fmt;
use uuid::Uuid;

pub const IDENTITY_FORMAT_VERSION: i64 = 1;

/// UTF-8 compact serde_json encoding, recursively sorted keys, original array order.
fn canonical_plan_bytes(
    definition: &impl Serialize,
    execution_order: &[String],
) -> Result<Vec<u8>, ContractError> {
    let definition = serde_json::to_value(definition)?;
    let mut value = json!({
        "identity_version": IDENTITY_FORMAT_VERSION,
        "definition": definition,
        "execution_order": execution_order,
    });
    // Explicit sorting also works when a downstream crate enables preserve_order.
    value.sort_all_objects();
    Ok(serde_json::to_vec(&value)?)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WorkflowId(String);

impl WorkflowId {
    pub fn from_definition(
        definition: &impl Serialize,
        execution_order: &[String],
    ) -> Result<Self, ContractError> {
        let bytes = canonical_plan_bytes(definition, execution_order)?;
        let hex: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(Self(format!("sha256:{hex}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for WorkflowId {
    type Error = ContractError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        require(
            value.strip_prefix("sha256:").is_some_and(|hex| {
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }),
            "workflow ID must be sha256: followed by 64 lowercase hexadecimal digits",
        )?;
        Ok(Self(value))
    }
}

impl From<WorkflowId> for String {
    fn from(value: WorkflowId) -> Self {
        value.0
    }
}

impl fmt::Display for WorkflowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RunId(Uuid);

impl RunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl TryFrom<String> for RunId {
    type Error = ContractError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let uuid = Uuid::parse_str(&value).context(crate::RunIdSnafu)?;
        require(
            uuid.get_version_num() == 4 && uuid.get_variant() == uuid::Variant::RFC4122,
            "run ID must be an RFC 4122 UUID v4",
        )?;
        require(
            uuid.to_string() == value,
            "run ID must use canonical lowercase UUID spelling",
        )?;
        Ok(Self(uuid))
    }
}

impl From<RunId> for String {
    fn from(value: RunId) -> Self {
        value.0.to_string()
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
