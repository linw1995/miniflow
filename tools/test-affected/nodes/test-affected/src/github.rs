use crate::{
    error::*,
    evidence::{Budget, EvidenceOutput},
};
use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeMetadata, NodeRegistration,
    NodeValue, PreparedNode, TypedNodeResult, TypedTaskNode, ValueRef, deserialize_config,
};
use serde::Deserialize;
use serde_json::{Value, json};
use snafu::{ResultExt, ensure};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
};

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    api_base: String,
    required: bool,
    timeout_ms: u64,
    max_bytes: usize,
    max_value_bytes: usize,
    member: Option<String>,
    pointer: String,
    fields: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_base: "https://api.github.com/".into(),
            required: true,
            timeout_ms: 30_000,
            max_bytes: 8 * 1024 * 1024,
            max_value_bytes: 256 * 1024,
            member: None,
            pointer: String::new(),
            fields: BTreeMap::new(),
        }
    }
}

#[derive(NodeValue)]
struct Input {
    path: String,
    query: Option<ValueRef>,
    token: Option<mf_runtime::Nullable<String>>,
    deadline: Option<u64>,
}

struct GithubRead {
    config: Config,
    client: reqwest::Client,
}

impl GithubRead {
    async fn read(&self, input: &Input) -> Result<Value, Error> {
        let mut request = self
            .client
            .get(format!(
                "{}{}",
                self.config.api_base,
                input.path.trim_start_matches('/')
            ))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(mf_runtime::Nullable::Value(token)) = &input.token
            && !token.is_empty()
        {
            request = request.bearer_auth(token);
        }
        if let Some(query) = &input.query {
            let query = serde_json::to_value(query).context(JsonSnafu {
                action: "encode API query",
            })?;
            ensure!(
                query.as_object().is_some_and(|q| q
                    .values()
                    .all(|v| v.is_string() || v.is_number() || v.is_boolean())),
                InvalidSnafu {
                    message: "API query must be an object of scalar values"
                }
            );
            request = request.query(&query);
        }
        // Redirect errors may contain signed artifact URLs; retain the typed source without its URL.
        let mut response = request
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context(HttpSnafu)?;
        ensure!(
            response.status().is_success(),
            ResponseSnafu {
                status: response.status().as_u16()
            }
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|n| n <= self.config.max_bytes as u64),
            LimitSnafu {
                message: "response body too large"
            }
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(reqwest::Error::without_url)
            .context(HttpSnafu)?
        {
            ensure!(
                bytes.len().saturating_add(chunk.len()) <= self.config.max_bytes,
                LimitSnafu {
                    message: "response body too large"
                }
            );
            bytes.extend_from_slice(&chunk);
        }
        let mut members = Vec::new();
        if let Some(member) = &self.config.member {
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).context(ArchiveSnafu)?;
            ensure!(
                archive.len() <= 256,
                LimitSnafu {
                    message: "too many artifact entries"
                }
            );
            for name in archive.file_names() {
                members.push(name.context(ArchiveSnafu)?.into_owned());
            }
            let file = archive.by_name(member).context(ArchiveSnafu)?;
            ensure!(
                file.size() <= self.config.max_value_bytes as u64,
                LimitSnafu {
                    message: "artifact member too large"
                }
            );
            let mut decoded = Vec::new();
            file.take(self.config.max_value_bytes as u64 + 1)
                .read_to_end(&mut decoded)
                .context(IoSnafu {
                    action: "read artifact member",
                })?;
            ensure!(
                decoded.len() <= self.config.max_value_bytes,
                LimitSnafu {
                    message: "artifact member too large"
                }
            );
            bytes = decoded;
        }
        let raw: Value = serde_json::from_slice(&bytes).context(JsonSnafu {
            action: "decode GitHub evidence",
        })?;
        let selected = raw.pointer(&self.config.pointer).unwrap_or(&Value::Null);
        let project = |value: &Value| -> Value {
            if self.config.fields.is_empty() {
                return value.clone();
            }
            Value::Object(
                self.config
                    .fields
                    .iter()
                    .map(|(name, path)| {
                        (
                            name.clone(),
                            value.pointer(path).cloned().unwrap_or(Value::Null),
                        )
                    })
                    .collect(),
            )
        };
        let data = if selected.is_array() && !self.config.fields.is_empty() {
            Value::Array(selected.as_array().unwrap().iter().map(project).collect())
        } else {
            project(selected)
        };
        let value = json!({"data":data,"members":members});
        ensure!(
            serde_json::to_vec(&value)
                .context(JsonSnafu {
                    action: "measure GitHub evidence"
                })?
                .len()
                <= self.config.max_value_bytes,
            LimitSnafu {
                message: "projected evidence too large"
            }
        );
        Ok(value)
    }
}

impl TypedTaskNode for GithubRead {
    type Input = Input;
    type Output = EvidenceOutput;
    fn execute(
        &self,
        input: Input,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<EvidenceOutput>, NodeExecutionError> {
        ensure!(
            input.path.starts_with('/')
                && !input.path.starts_with("//")
                && !input.path.contains(['?', '#', '\\']),
            InvalidSnafu {
                message: "API path must be absolute within the configured host; supply query separately"
            }
        );
        let budget = Budget::new(input.deadline, self.config.timeout_ms)?;
        let result = budget
            .check(context)
            .and_then(|()| {
                context
                    .execute_future(self.read(&input), Some(budget.instant))
                    .map_err(Error::from)
                    .and_then(|result| result)
            })
            .and_then(|value| {
                budget.check(context)?;
                Ok(value)
            });
        Ok(budget.outcome(result, self.config.required)?.into())
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let config: Config = deserialize_config(config)?;
    let base = url::Url::parse(&config.api_base).context(UrlSnafu)?;
    ensure!(
        config.api_base.ends_with('/')
            && matches!(base.scheme(), "http" | "https")
            && base.host_str().is_some()
            && base.username().is_empty()
            && base.password().is_none()
            && base.query().is_none()
            && base.fragment().is_none(),
        InvalidSnafu {
            message: "api_base must be an HTTP base URL ending with / and contain no credentials, query, or fragment"
        }
    );
    ensure!(
        std::iter::once(&config.pointer)
            .chain(config.fields.values())
            .all(|p| p.is_empty() || p.starts_with('/')),
        InvalidSnafu {
            message: "projection paths must be JSON Pointers"
        }
    );
    ensure!(
        (1..=16 * 1024 * 1024).contains(&config.max_bytes)
            && (1..=512 * 1024).contains(&config.max_value_bytes),
        InvalidSnafu {
            message: "invalid evidence byte limits"
        }
    );
    Budget::new(None, config.timeout_ms)?;
    let client = reqwest::Client::builder()
        .user_agent("miniflow-developer-tools")
        .build()
        .context(HttpSnafu)?;
    PreparedNode::typed_task(GithubRead { config, client }, NodeMetadata::default())
}

inventory::submit! { NodeRegistration { kind:"github.read", factory:mf_runtime::NodeFactory::Plain(factory) } }
