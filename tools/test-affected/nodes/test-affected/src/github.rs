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
    path::Path,
};

fn validate_api_base(api_base: &str) -> Result<(), Error> {
    let base = url::Url::parse(api_base).context(UrlSnafu)?;
    ensure!(
        api_base.ends_with('/')
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
    Ok(())
}

fn validate_path(path: &str) -> Result<(), Error> {
    ensure!(
        path.starts_with('/') && !path.starts_with("//") && !path.contains(['?', '#', '\\']),
        InvalidSnafu {
            message: "API path must be absolute within the configured host; supply query separately"
        }
    );
    Ok(())
}

/// GETs one bounded response body through the authenticated GitHub channel.
async fn fetch(
    client: &reqwest::Client,
    api_base: &str,
    path: &str,
    token: Option<&str>,
    query: Option<&Value>,
    max_bytes: usize,
) -> Result<Vec<u8>, Error> {
    let mut request = client
        .get(format!("{}{}", api_base, path.trim_start_matches('/')))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(token) = token.filter(|token| !token.is_empty()) {
        request = request.bearer_auth(token);
    }
    if let Some(query) = query {
        ensure!(
            query.as_object().is_some_and(|q| q
                .values()
                .all(|v| v.is_string() || v.is_number() || v.is_boolean())),
            InvalidSnafu {
                message: "API query must be an object of scalar values"
            }
        );
        request = request.query(query);
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
            .is_none_or(|n| n <= max_bytes as u64),
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
            bytes.len().saturating_add(chunk.len()) <= max_bytes,
            LimitSnafu {
                message: "response body too large"
            }
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

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
        let query = input
            .query
            .as_ref()
            .map(|query| {
                serde_json::to_value(query).context(JsonSnafu {
                    action: "encode API query",
                })
            })
            .transpose()?;
        let token = match &input.token {
            Some(mf_runtime::Nullable::Value(token)) => Some(token.as_str()),
            _ => None,
        };
        let mut bytes = fetch(
            &self.client,
            &self.config.api_base,
            &input.path,
            token,
            query.as_ref(),
            self.config.max_bytes,
        )
        .await?;
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
        validate_path(&input.path)?;
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
    validate_api_base(&config.api_base)?;
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

/// Downloads one artifact archive and materializes its configured members on
/// disk for file-based consumers such as the coverage graph reader. The
/// `metadata_member`, when configured, is additionally parsed as bounded JSON
/// and returned for in-flow validation.
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DownloadConfig {
    api_base: String,
    required: bool,
    timeout_ms: u64,
    max_bytes: usize,
    max_member_bytes: usize,
    members: Vec<String>,
    metadata_member: Option<String>,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            api_base: "https://api.github.com/".into(),
            required: true,
            timeout_ms: 30_000,
            max_bytes: 256 * 1024 * 1024,
            max_member_bytes: 64 * 1024 * 1024,
            members: Vec::new(),
            metadata_member: None,
        }
    }
}

#[derive(NodeValue)]
struct DownloadInput {
    path: String,
    directory: String,
    token: Option<mf_runtime::Nullable<String>>,
    deadline: Option<u64>,
}

struct GithubDownload {
    config: DownloadConfig,
    client: reqwest::Client,
}

impl GithubDownload {
    async fn download(&self, input: &DownloadInput) -> Result<Value, Error> {
        let token = match &input.token {
            Some(mf_runtime::Nullable::Value(token)) => Some(token.as_str()),
            _ => None,
        };
        let bytes = fetch(
            &self.client,
            &self.config.api_base,
            &input.path,
            token,
            None,
            self.config.max_bytes,
        )
        .await?;
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).context(ArchiveSnafu)?;
        ensure!(
            archive.len() <= 256,
            LimitSnafu {
                message: "too many artifact entries"
            }
        );
        let directory = Path::new(&input.directory);
        std::fs::create_dir_all(directory).context(IoSnafu {
            action: "create artifact directory",
        })?;
        let mut extracted = Vec::new();
        for member in &self.config.members {
            let file = archive.by_name(member).context(ArchiveSnafu)?;
            ensure!(
                file.size() <= self.config.max_member_bytes as u64,
                LimitSnafu {
                    message: "artifact member too large"
                }
            );
            let mut contents = Vec::new();
            file.take(self.config.max_member_bytes as u64 + 1)
                .read_to_end(&mut contents)
                .context(IoSnafu {
                    action: "read artifact member",
                })?;
            ensure!(
                contents.len() <= self.config.max_member_bytes,
                LimitSnafu {
                    message: "artifact member too large"
                }
            );
            std::fs::write(directory.join(member), contents).context(IoSnafu {
                action: "write artifact member",
            })?;
            extracted.push(member.clone());
        }
        let metadata = match &self.config.metadata_member {
            Some(member) => {
                let text = std::fs::read(directory.join(member)).context(IoSnafu {
                    action: "read artifact metadata",
                })?;
                ensure!(
                    text.len() <= 256 * 1024,
                    LimitSnafu {
                        message: "artifact metadata too large"
                    }
                );
                serde_json::from_slice(&text).context(JsonSnafu {
                    action: "decode artifact metadata",
                })?
            }
            None => Value::Null,
        };
        Ok(json!({"members":extracted,"metadata":metadata}))
    }
}

impl TypedTaskNode for GithubDownload {
    type Input = DownloadInput;
    type Output = EvidenceOutput;
    fn execute(
        &self,
        input: DownloadInput,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<EvidenceOutput>, NodeExecutionError> {
        validate_path(&input.path)?;
        ensure!(
            !input.directory.is_empty() && Path::new(&input.directory).is_absolute(),
            InvalidSnafu {
                message: "artifact directory must be an absolute path"
            }
        );
        let budget = Budget::new(input.deadline, self.config.timeout_ms)?;
        let result = budget
            .check(context)
            .and_then(|()| {
                context
                    .execute_future(self.download(&input), Some(budget.instant))
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

fn download_factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let config: DownloadConfig = deserialize_config(config)?;
    validate_api_base(&config.api_base)?;
    ensure!(
        !config.members.is_empty()
            && config.members.iter().all(|member| {
                !member.is_empty()
                    && !member.contains(['/', '\\'])
                    && member != "."
                    && member != ".."
            }),
        InvalidSnafu {
            message: "members must be plain archive file names"
        }
    );
    ensure!(
        config
            .metadata_member
            .as_ref()
            .is_none_or(|member| config.members.contains(member)),
        InvalidSnafu {
            message: "metadata_member must be one of the extracted members"
        }
    );
    ensure!(
        (1..=1024 * 1024 * 1024).contains(&config.max_bytes)
            && (1..=256 * 1024 * 1024).contains(&config.max_member_bytes),
        InvalidSnafu {
            message: "invalid artifact byte limits"
        }
    );
    Budget::new(None, config.timeout_ms)?;
    let client = reqwest::Client::builder()
        .user_agent("miniflow-developer-tools")
        .build()
        .context(HttpSnafu)?;
    PreparedNode::typed_task(GithubDownload { config, client }, NodeMetadata::default())
}

inventory::submit! { NodeRegistration { kind:"github.download", factory:mf_runtime::NodeFactory::Plain(download_factory) } }
