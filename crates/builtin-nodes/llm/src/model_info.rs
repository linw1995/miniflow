//! Read-only model metadata discovery over OpenAI-compatible `/models` endpoints.

use crate::{
    ClientInitializationSnafu, ConfigTypeSnafu, DeadlineRangeSnafu, DecodeSnafu,
    InvalidConfigSnafu, InvalidResponseSnafu, LlmConfigError, LlmExecutionError,
    ModelInfoResponseSnafu, ProviderSnafu, TemplateSnafu, build_http_client, resolve_api_key,
    validate_base_url, validate_field,
};
use bytes::Bytes;
use futures::StreamExt;
use mf_runtime::{
    ConfigTemplate, ContextReference, ExecutionContext, Inputs, NodeBuildError, NodeExecutionError,
    NodeFactory, NodeMetadata, NodeRegistration, NodeValue, PreparedNode, Shared, TypedNodeResult,
    TypedTaskNode, deserialize_config,
};
use rig_core::{
    error::ProviderError,
    http_client::{HttpClientExt, Request},
};
use rig_reqwest::ReqwestClient;
use serde_json::Value;
use snafu::{IntoError, OptionExt, ResultExt, ensure};
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

pub const MODEL_INFO_KIND: &str = "llm.model_info";

const DEFAULT_TIMEOUT_MS: u64 = 60_000;
const MAX_MODELS_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Read-only model metadata normalized across OpenAI-compatible `/models` dialects.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, NodeValue)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<i64>,
}

/// Parameters for a direct read-only `/models` lookup.
#[derive(Clone, Debug)]
pub struct ModelInfoQuery {
    pub model: String,
    pub base_url: String,
    /// When `None` or empty, the request omits the `Authorization` header.
    pub api_key: Option<String>,
    pub timeout_ms: u64,
}

impl ModelInfoQuery {
    pub fn new(model: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            base_url: base_url.into(),
            api_key: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }
}

#[derive(Clone, Debug, Default, NodeValue)]
pub struct ModelInfoInputs {}

#[derive(Clone, Debug, PartialEq, Eq, NodeValue)]
pub struct ModelInfoOutputs {
    pub status: String,
    pub found: bool,
    pub model: String,
    pub info: ModelInfo,
    pub context_window: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub error: Option<String>,
}

#[derive(mf_runtime::NodeInputs)]
struct ModelInfoConfig {
    model: String,
    base_url: String,
    api_key_env: String,
    api_key: Option<Shared<String>>,
    timeout_ms: u64,
    capture_errors: bool,
}

struct PreparedModelInfoConfig {
    fields: BTreeMap<String, ConfigTemplate>,
}

impl PreparedModelInfoConfig {
    fn parse(value: Value) -> Result<Self, NodeBuildError> {
        let mut fields: BTreeMap<String, Value> = deserialize_config(value)?;
        ensure!(
            fields.contains_key("model"),
            InvalidConfigSnafu {
                field: "model",
                requirement: "a model identifier or template",
            }
        );
        ensure!(
            !(fields.contains_key("api_key") && fields.contains_key("api_key_env")),
            InvalidConfigSnafu {
                field: "api_key",
                requirement: "exactly one credential source",
            }
        );
        for (name, value) in [
            ("base_url", Value::from("https://api.openai.com/v1")),
            ("api_key_env", Value::from("OPENAI_API_KEY")),
            ("timeout_ms", Value::from(DEFAULT_TIMEOUT_MS)),
            ("capture_errors", Value::from(false)),
        ] {
            fields.entry(name.into()).or_insert(value);
        }
        let ports = <ModelInfoConfig as mf_runtime::NodeInputs>::ports();
        let mut templates = BTreeMap::new();
        for (field, value) in fields {
            let port =
                ports
                    .iter()
                    .find(|port| port.name == field)
                    .context(InvalidConfigSnafu {
                        field: &field,
                        requirement: "a supported LLM model info configuration field",
                    })?;
            let template = ConfigTemplate::parse(value).context(TemplateSnafu { field: &field })?;
            if field == "api_key" {
                ensure!(
                    !template.references(&field).is_empty(),
                    InvalidConfigSnafu {
                        field: &field,
                        requirement: "a runtime template instead of a literal credential",
                    }
                );
            }
            if field == "api_key_env" {
                ensure!(
                    template.literal().is_some(),
                    InvalidConfigSnafu {
                        field: &field,
                        requirement: "a literal environment-variable name",
                    }
                );
            }
            if let Some(value) = template.literal() {
                port.value_type
                    .validate_shared(value)
                    .context(ConfigTypeSnafu { field: &field })?;
                validate_field(&field, value)?;
            }
            templates.insert(field, template);
        }
        Ok(Self { fields: templates })
    }

    fn references(&self) -> Vec<ContextReference> {
        self.fields
            .iter()
            .flat_map(|(field, value)| value.references(field))
            .collect()
    }

    fn resolve(&self, context: &ExecutionContext) -> Result<ModelInfoConfig, LlmConfigError> {
        let mut fields = Inputs::new();
        for (field, template) in &self.fields {
            let value = template.resolve(context).context(TemplateSnafu { field })?;
            let value = if field == "api_key" {
                value.into_secret()
            } else {
                value
            };
            fields.insert(field.clone(), value);
        }
        let mut config = <ModelInfoConfig as mf_runtime::NodeInputs>::from_inputs(fields.clone())
            .context(DecodeSnafu)?;
        for (field, value) in &fields {
            validate_field(field, value)?;
        }
        config.base_url = validate_base_url("base_url", &config.base_url)?;
        Ok(config)
    }
}

/// Validates static literals and templates for `llm.model_info` without external I/O.
pub fn validate_model_info_config(value: Value) -> Result<(), NodeBuildError> {
    PreparedModelInfoConfig::parse(value).map(|_| ())
}

fn shared_client() -> Result<ReqwestClient, LlmExecutionError> {
    static CLIENT: OnceLock<Result<ReqwestClient, Arc<reqwest::Error>>> = OnceLock::new();
    CLIENT
        .get_or_init(|| build_http_client().map_err(Arc::new))
        .as_ref()
        .cloned()
        .map_err(Arc::clone)
        .context(ClientInitializationSnafu)
}

/// Queries `GET {base_url}/models` once and returns normalized metadata for `query.model`.
///
/// Returns `Ok(None)` when the endpoint returns a valid listing that does not include
/// `query.model`. Missing optional metadata fields (such as `context_window`) are returned
/// as `None` inside `ModelInfo`.
pub fn query_model_info(
    query: &ModelInfoQuery,
    context: &ExecutionContext,
) -> Result<Option<ModelInfo>, LlmExecutionError> {
    let client = shared_client()?;
    query_model_info_with_client(&client, query, context)
}

fn query_model_info_with_client(
    client: &ReqwestClient,
    query: &ModelInfoQuery,
    context: &ExecutionContext,
) -> Result<Option<ModelInfo>, LlmExecutionError> {
    ensure!(
        !query.model.trim().is_empty(),
        InvalidConfigSnafu {
            field: "model",
            requirement: "a nonblank identifier",
        }
    );
    ensure!(
        query.timeout_ms > 0,
        InvalidConfigSnafu {
            field: "timeout_ms",
            requirement: "a positive integer",
        }
    );
    let base_url = validate_base_url("base_url", &query.base_url)?;
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(query.timeout_ms))
        .context(DeadlineRangeSnafu)?;
    context.check_future_limits(Some(deadline))?;
    let endpoint = format!("{base_url}/models");
    let api_key = query
        .api_key
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let model = query.model.clone();
    let client = client.clone();
    let result = context.execute_future(
        fetch_model_info(client, endpoint, api_key, model),
        Some(deadline),
    )??;
    context.check_future_limits(Some(deadline))?;
    Ok(result)
}

fn transport_provider_error(error: rig_core::http_client::Error) -> LlmExecutionError {
    let source = ProviderError::from_transport_error(error);
    ProviderSnafu {
        kind: source.kind().code(),
        status: source
            .provider_response_status()
            .map(|status| status.as_u16()),
    }
    .into_error(source)
}

async fn fetch_model_info(
    client: ReqwestClient,
    endpoint: String,
    api_key: Option<String>,
    model: String,
) -> Result<Option<ModelInfo>, LlmExecutionError> {
    let mut builder = Request::get(&endpoint).header("accept", "application/json");
    if let Some(key) = api_key {
        builder = builder.header("authorization", format!("Bearer {key}"));
    }
    let request = builder
        .body(Bytes::new())
        .map_err(|error| transport_provider_error(rig_core::http_client::Error::Protocol(error)))?;
    let response = client
        .send_streaming(request)
        .await
        .map_err(transport_provider_error)?;
    let mut stream = response.into_body();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(transport_provider_error)?;
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= MAX_MODELS_RESPONSE_BYTES,
            InvalidResponseSnafu {
                requirement: "a model listing within the size limit",
            }
        );
        bytes.extend_from_slice(&chunk);
    }
    parse_models_listing(&bytes, &model)
}

fn parse_models_listing(bytes: &[u8], model: &str) -> Result<Option<ModelInfo>, LlmExecutionError> {
    let root: Value = serde_json::from_slice(bytes).context(ModelInfoResponseSnafu)?;
    let data = root
        .get("data")
        .and_then(Value::as_array)
        .context(InvalidResponseSnafu {
            requirement: "a JSON object with a data array",
        })?;
    Ok(data
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some(model))
        .map(|entry| extract_model_info(entry, model)))
}

fn extract_model_info(entry: &Value, model_id: &str) -> ModelInfo {
    let top_provider = entry.get("top_provider");
    let context_window = [
        entry.get("context_window"),
        entry.get("context_length"),
        entry.get("max_context_length"),
        entry.get("max_model_len"),
        entry.get("max_input_tokens"),
        top_provider.and_then(|provider| provider.get("context_length")),
    ]
    .into_iter()
    .flatten()
    .find_map(positive_i64);
    let max_output_tokens = [
        entry.get("max_completion_tokens"),
        entry.get("max_output_tokens"),
        top_provider.and_then(|provider| provider.get("max_completion_tokens")),
        top_provider.and_then(|provider| provider.get("max_output_tokens")),
    ]
    .into_iter()
    .flatten()
    .find_map(positive_i64);
    let name = entry
        .get("name")
        .or_else(|| entry.get("display_name"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let owned_by = entry
        .get("owned_by")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let created_at = entry
        .get("created")
        .or_else(|| entry.get("created_at"))
        .and_then(nonnegative_i64);
    ModelInfo {
        id: model_id.to_owned(),
        name,
        owned_by,
        created_at,
        context_window,
        max_output_tokens,
    }
}

fn positive_i64(value: &Value) -> Option<i64> {
    value.as_i64().filter(|&number| number > 0).or_else(|| {
        value
            .as_u64()
            .and_then(|number| i64::try_from(number).ok())
            .filter(|&number| number > 0)
    })
}

fn nonnegative_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .filter(|&number| number >= 0)
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
}

struct ModelInfoNode {
    config: PreparedModelInfoConfig,
    client: OnceLock<Result<ReqwestClient, Arc<reqwest::Error>>>,
}

impl ModelInfoNode {
    fn client(&self) -> Result<ReqwestClient, LlmExecutionError> {
        self.client
            .get_or_init(|| build_http_client().map_err(Arc::new))
            .as_ref()
            .cloned()
            .map_err(Arc::clone)
            .context(ClientInitializationSnafu)
    }

    fn execute_lookup(
        &self,
        context: &ExecutionContext,
    ) -> Result<ModelInfoOutputs, LlmExecutionError> {
        let config = self.config.resolve(context)?;
        let key = resolve_api_key(config.api_key.as_ref(), &config.api_key_env)?;
        let query = ModelInfoQuery {
            model: config.model.clone(),
            base_url: config.base_url,
            api_key: (!key.is_empty()).then_some(key),
            timeout_ms: config.timeout_ms,
        };
        let client = self.client()?;
        match query_model_info_with_client(&client, &query, context) {
            Ok(Some(info)) => Ok(ModelInfoOutputs {
                status: "succeeded".into(),
                found: true,
                model: info.id.clone(),
                context_window: info.context_window,
                max_output_tokens: info.max_output_tokens,
                info,
                error: None,
            }),
            Ok(None) => Ok(ModelInfoOutputs {
                status: "not_found".into(),
                found: false,
                model: config.model.clone(),
                info: ModelInfo {
                    id: config.model,
                    ..Default::default()
                },
                context_window: None,
                max_output_tokens: None,
                error: None,
            }),
            Err(error) if config.capture_errors => {
                context.check_future_limits(None)?;
                Ok(ModelInfoOutputs {
                    status: "failed".into(),
                    found: false,
                    model: config.model.clone(),
                    info: ModelInfo {
                        id: config.model,
                        ..Default::default()
                    },
                    context_window: None,
                    max_output_tokens: None,
                    error: Some(error.to_string()),
                })
            }
            Err(error) => Err(error),
        }
    }
}

impl TypedTaskNode for ModelInfoNode {
    type Input = ModelInfoInputs;
    type Output = ModelInfoOutputs;

    fn execute(
        &self,
        _: Self::Input,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        let outputs = self.execute_lookup(context)?;
        let skipped = [
            ("context_window", outputs.context_window.is_none()),
            ("max_output_tokens", outputs.max_output_tokens.is_none()),
            ("error", outputs.error.is_none()),
        ]
        .into_iter()
        .filter(|(_, absent)| *absent)
        .map(|(name, _)| name.to_owned())
        .collect();
        Ok(TypedNodeResult {
            outputs,
            skipped,
            loop_summary: None,
        })
    }
}

/// Prepares the read-only `llm.model_info` task node without acquiring network resources.
pub fn prepare_model_info(value: Value) -> Result<PreparedNode, NodeBuildError> {
    let config = PreparedModelInfoConfig::parse(value)?;
    let metadata = NodeMetadata {
        context_references: config.references(),
        ..Default::default()
    };
    PreparedNode::typed_task(
        ModelInfoNode {
            config,
            client: OnceLock::new(),
        },
        metadata,
    )
}

inventory::submit! {
    NodeRegistration {
        kind: MODEL_INFO_KIND,
        factory: NodeFactory::Plain(prepare_model_info),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::NodeValue;
    use serde_json::json;

    #[test]
    fn model_info_config_validates_defaults_and_rejects_invalid_fields() {
        let config = PreparedModelInfoConfig::parse(json!({"model": "example-model"}))
            .unwrap()
            .resolve(&ExecutionContext::default())
            .unwrap();
        assert_eq!(config.model, "example-model");
        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert_eq!(config.api_key_env, "OPENAI_API_KEY");
        assert_eq!(config.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert!(!config.capture_errors);

        for invalid in [
            json!({}),
            json!({"model": " "}),
            json!({"model": "example", "api_key": "literal-secret"}),
            json!({"model": "example", "api_key": {"$ref": "params.k"}, "api_key_env": "KEY"}),
            json!({"model": "example", "base_url": "file:///tmp/models"}),
            json!({"model": "example", "base_url": "https://user:pass@host/v1"}),
            json!({"model": "example", "timeout_ms": 0}),
            json!({"model": "example", "temperature": 0.5}),
        ] {
            assert!(
                validate_model_info_config(invalid.clone()).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn parse_models_listing_normalizes_context_window_across_provider_dialects() {
        let payload = json!({
            "object": "list",
            "data": [
                {
                    "id": "groq-model",
                    "owned_by": "Meta",
                    "created": 1700000000,
                    "context_window": 131072,
                    "max_completion_tokens": 32768
                },
                {
                    "id": "openrouter/claude",
                    "name": "Claude",
                    "context_length": 200000,
                    "top_provider": {"context_length": 100000, "max_completion_tokens": 16384}
                },
                {
                    "id": "openrouter/top-provider-only",
                    "context_length": 0,
                    "top_provider": {"context_length": 65536, "max_output_tokens": 8192}
                },
                {
                    "id": "mistral-large",
                    "max_context_length": 128000
                },
                {
                    "id": "vllm-served",
                    "max_model_len": 32768
                },
                {
                    "id": "gateway-model",
                    "display_name": "Gateway Model",
                    "max_input_tokens": 64000,
                    "max_output_tokens": 4096
                },
                {
                    "id": "gpt-4o",
                    "object": "model",
                    "created": 1715367049,
                    "owned_by": "system"
                }
            ]
        })
        .to_string();

        let groq = parse_models_listing(payload.as_bytes(), "groq-model")
            .unwrap()
            .unwrap();
        assert_eq!(groq.context_window, Some(131_072));
        assert_eq!(groq.max_output_tokens, Some(32_768));
        assert_eq!(groq.owned_by.as_deref(), Some("Meta"));
        assert_eq!(groq.created_at, Some(1_700_000_000));

        let openrouter = parse_models_listing(payload.as_bytes(), "openrouter/claude")
            .unwrap()
            .unwrap();
        assert_eq!(openrouter.name.as_deref(), Some("Claude"));
        assert_eq!(openrouter.context_window, Some(200_000));
        assert_eq!(openrouter.max_output_tokens, Some(16_384));

        let top_provider = parse_models_listing(payload.as_bytes(), "openrouter/top-provider-only")
            .unwrap()
            .unwrap();
        assert_eq!(top_provider.context_window, Some(65_536));
        assert_eq!(top_provider.max_output_tokens, Some(8_192));

        assert_eq!(
            parse_models_listing(payload.as_bytes(), "mistral-large")
                .unwrap()
                .unwrap()
                .context_window,
            Some(128_000)
        );
        assert_eq!(
            parse_models_listing(payload.as_bytes(), "vllm-served")
                .unwrap()
                .unwrap()
                .context_window,
            Some(32_768)
        );
        let gateway = parse_models_listing(payload.as_bytes(), "gateway-model")
            .unwrap()
            .unwrap();
        assert_eq!(gateway.name.as_deref(), Some("Gateway Model"));
        assert_eq!(gateway.context_window, Some(64_000));
        assert_eq!(gateway.max_output_tokens, Some(4_096));

        let openai = parse_models_listing(payload.as_bytes(), "gpt-4o")
            .unwrap()
            .unwrap();
        assert_eq!(openai.context_window, None);
        assert_eq!(openai.max_output_tokens, None);
        assert_eq!(openai.owned_by.as_deref(), Some("system"));

        assert_eq!(
            parse_models_listing(payload.as_bytes(), "missing-model").unwrap(),
            None
        );
    }

    #[test]
    fn parse_models_listing_rejects_malformed_payloads() {
        assert!(matches!(
            parse_models_listing(b"not-json", "m"),
            Err(LlmExecutionError::ModelInfoResponse { .. })
        ));
        assert!(matches!(
            parse_models_listing(br#"{"object":"list"}"#, "m"),
            Err(LlmExecutionError::InvalidResponse { .. })
        ));
    }

    #[test]
    fn prepared_model_info_reflects_ports_without_initializing_client() {
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let prepared = registry
            .get(MODEL_INFO_KIND)
            .unwrap()
            .instantiate(json!({
                "model": "example-model",
                "base_url": "http://127.0.0.1:9/v1",
                "api_key_env": "MINIFLOW_LLM_UNAVAILABLE_CREDENTIAL"
            }))
            .unwrap();
        assert_eq!(prepared.metadata.ports.inputs, ModelInfoInputs::ports());
        assert_eq!(prepared.metadata.ports.outputs, ModelInfoOutputs::ports());
        assert!(prepared.execution.as_task_node().is_some());
    }
}
