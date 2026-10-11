#[cfg(feature = "test-support")]
#[path = "../tests/support/mod.rs"]
pub mod test_support;
mod transport;

use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodeRegistration, NodeValue, PreparedNode, TypedNodeResult, TypedTaskNode, deserialize_config,
};
use rig_core::{
    completion::{AssistantContent, CompletionRequest, CompletionResponse, FinishReason, Message},
    error::ProviderError,
    providers::openai::OpenAIConfig,
};
use rig_reqwest::ReqwestClient;
use serde::Deserialize;
use serde_json::Value;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

pub const KIND: &str = "builtin.llm";

#[derive(Clone, Debug, PartialEq, Eq, NodeValue)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, NodeValue)]
pub struct LlmInputs {
    pub prompt: String,
    pub system: Option<String>,
    pub history: Option<Vec<ChatMessage>>,
}

#[derive(Clone, Debug, NodeValue)]
pub struct LlmOutputs {
    pub text: String,
    pub history: Vec<ChatMessage>,
    pub usage: Option<BTreeMap<String, i64>>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    model: String,
    #[serde(default = "default_base_url")]
    base_url: String,
    #[serde(default = "default_api_key_env")]
    api_key_env: String,
    #[serde(default, deserialize_with = "optional_value")]
    temperature: Option<f64>,
    #[serde(default, deserialize_with = "optional_value")]
    max_tokens: Option<u64>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_max_output_bytes")]
    max_output_bytes: usize,
}

fn default_base_url() -> String {
    "https://api.openai.com/v1".into()
}
fn default_api_key_env() -> String {
    "OPENAI_API_KEY".into()
}
fn default_timeout_ms() -> u64 {
    60_000
}
fn default_max_output_bytes() -> usize {
    1_048_576
}

fn optional_value<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Snafu)]
pub enum LlmBuildError {
    #[snafu(display("configuration `{field}` requires {requirement}"))]
    InvalidConfig {
        field: &'static str,
        requirement: &'static str,
    },
    #[snafu(display("configuration `base_url` is not a valid absolute URL: {source}"))]
    BaseUrl { source: url::ParseError },
}

impl From<LlmBuildError> for NodeBuildError {
    fn from(source: LlmBuildError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

fn parse_config(value: Value) -> Result<Config, NodeBuildError> {
    let mut config: Config = deserialize_config(value)?;
    ensure!(
        !config.model.trim().is_empty(),
        InvalidConfigSnafu {
            field: "model",
            requirement: "a nonblank identifier"
        }
    );
    let base = url::Url::parse(&config.base_url).context(BaseUrlSnafu)?;
    ensure!(
        matches!(base.scheme(), "http" | "https") && base.host_str().is_some(),
        InvalidConfigSnafu {
            field: "base_url",
            requirement: "an absolute HTTP or HTTPS URL"
        }
    );
    ensure!(
        base.username().is_empty()
            && base.password().is_none()
            && base.query().is_none()
            && base.fragment().is_none(),
        InvalidConfigSnafu {
            field: "base_url",
            requirement: "no embedded credentials, query, or fragment"
        }
    );
    config.base_url = base.as_str().trim_end_matches('/').to_owned();
    ensure!(
        !config.api_key_env.trim().is_empty() && !config.api_key_env.contains(['=', '\0']),
        InvalidConfigSnafu {
            field: "api_key_env",
            requirement: "a nonblank valid environment-variable name"
        }
    );
    ensure!(
        config
            .temperature
            .is_none_or(|value| value.is_finite() && (0.0..=2.0).contains(&value)),
        InvalidConfigSnafu {
            field: "temperature",
            requirement: "a finite number from 0 to 2"
        }
    );
    ensure!(
        config.max_tokens.is_none_or(|value| value > 0),
        InvalidConfigSnafu {
            field: "max_tokens",
            requirement: "a positive integer"
        }
    );
    ensure!(
        config.timeout_ms > 0,
        InvalidConfigSnafu {
            field: "timeout_ms",
            requirement: "a positive integer"
        }
    );
    ensure!(
        config.max_output_bytes > 0,
        InvalidConfigSnafu {
            field: "max_output_bytes",
            requirement: "a positive integer"
        }
    );
    Ok(config)
}

#[derive(Snafu)]
pub enum LlmExecutionError {
    #[snafu(display("LLM input `{path}` requires {requirement}"))]
    InvalidInput {
        path: String,
        requirement: &'static str,
    },
    #[snafu(display("could not read LLM credential variable `{variable}`"))]
    Credentials {
        variable: String,
        source: std::env::VarError,
    },
    #[snafu(display("LLM credential variable `{variable}` is empty"))]
    EmptyCredentials { variable: String },
    #[snafu(display("could not initialize the LLM HTTP client"))]
    ClientInitialization { source: Arc<reqwest::Error> },
    #[snafu(display("LLM request failed ({kind}){}", status.map(|value| format!(": HTTP {value}")).unwrap_or_default()))]
    Provider {
        kind: &'static str,
        status: Option<u16>,
        #[snafu(source(from(ProviderError, Box::new)))]
        source: Box<ProviderError>,
    },
    #[snafu(display("LLM response contains unsupported nontext output"))]
    UnsupportedOutput,
    #[snafu(display("LLM response contains no nonempty text"))]
    EmptyText,
    #[snafu(display("LLM response text exceeds {limit} bytes"))]
    OutputTooLarge { limit: usize },
    #[snafu(display("LLM usage counter `{counter}` exceeds the signed integer range"))]
    UsageCount {
        counter: &'static str,
        source: std::num::TryFromIntError,
    },
    #[snafu(display("LLM timeout exceeds the monotonic clock range"))]
    DeadlineRange,
    #[snafu(display("LLM usage metadata is invalid"))]
    UsageMetadata { source: serde_json::Error },
    #[snafu(display("could not normalize LLM response metadata"))]
    NormalizeResponse { source: serde_json::Error },
    #[snafu(transparent)]
    Future {
        source: mf_runtime::FutureExecutionError,
    },
}

impl fmt::Debug for LlmExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Sources retain provider bodies and credentials for explicit inspection only.
        formatter
            .debug_struct("LlmExecutionError")
            .field("message", &format_args!("{self}"))
            .finish()
    }
}

impl From<LlmExecutionError> for NodeExecutionError {
    fn from(source: LlmExecutionError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

struct LlmNode {
    config: Config,
    client: OnceLock<Result<ReqwestClient, Arc<reqwest::Error>>>,
}

impl LlmNode {
    fn client(&self) -> Result<ReqwestClient, LlmExecutionError> {
        self.client
            .get_or_init(|| {
                reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .retry(reqwest::retry::never())
                    // A prepared node can outlive the reactor of an earlier execution scope.
                    .pool_max_idle_per_host(0)
                    .timeout(Duration::from_millis(self.config.timeout_ms))
                    .build()
                    .map(ReqwestClient::from)
                    .map_err(Arc::new)
            })
            .as_ref()
            .cloned()
            .map_err(Arc::clone)
            .context(ClientInitializationSnafu)
    }
}

impl TypedTaskNode for LlmNode {
    type Input = LlmInputs;
    type Output = LlmOutputs;

    fn execute(
        &self,
        inputs: Self::Input,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        self.complete(inputs, context)
            .map(Into::into)
            .map_err(Into::into)
    }
}

impl LlmNode {
    fn complete(
        &self,
        inputs: LlmInputs,
        context: &ExecutionContext,
    ) -> Result<LlmOutputs, LlmExecutionError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(self.config.timeout_ms))
            .context(DeadlineRangeSnafu)?;
        context.check_future_limits(Some(deadline))?;
        ensure!(
            !inputs.prompt.is_empty(),
            InvalidInputSnafu {
                path: "/prompt",
                requirement: "nonempty text"
            }
        );
        let history = inputs.history.unwrap_or_default();
        for (index, message) in history.iter().enumerate() {
            ensure!(
                matches!(message.role.as_str(), "user" | "assistant"),
                InvalidInputSnafu {
                    path: format!("/history/{index}/role"),
                    requirement: "role `user` or `assistant`"
                }
            );
            ensure!(
                !message.content.is_empty(),
                InvalidInputSnafu {
                    path: format!("/history/{index}/content"),
                    requirement: "nonempty text"
                }
            );
        }
        let key = std::env::var(&self.config.api_key_env).context(CredentialsSnafu {
            variable: &self.config.api_key_env,
        })?;
        ensure!(
            !key.is_empty(),
            EmptyCredentialsSnafu {
                variable: &self.config.api_key_env
            }
        );
        let transport = transport::TerminalTransport::new(self.client()?);
        let model = OpenAIConfig::new(key)
            .with_base_url(&self.config.base_url)
            .connect(transport.clone())
            .chat(&self.config.model);
        let messages = history.iter().map(|message| {
            if message.role == "user" {
                Message::user(&message.content)
            } else {
                Message::assistant(&message.content)
            }
        });
        let mut request = CompletionRequest::new(&inputs.prompt)
            .messages(messages)
            .temperature(self.config.temperature)
            .max_tokens(self.config.max_tokens);
        if let Some(system) = inputs.system {
            request = request.preamble(system);
        }
        let mut response = context
            .execute_future(model.call(request), Some(deadline))?
            .with_context(|source| ProviderSnafu {
                kind: source.kind().code(),
                status: source
                    .provider_response_status()
                    .map(|status| status.as_u16()),
            })?;
        if let Some(usage) = transport.usage() {
            response.usage = usage;
        }
        let result = terminal_outputs(
            response,
            inputs.prompt,
            history,
            self.config.max_output_bytes,
        )?;
        context.check_future_limits(Some(deadline))?;
        Ok(result)
    }
}

fn terminal_outputs(
    response: CompletionResponse,
    prompt: String,
    mut history: Vec<ChatMessage>,
    limit: usize,
) -> Result<LlmOutputs, LlmExecutionError> {
    let finish_reason = response.finish_reason().map(|reason| match reason {
        FinishReason::Stop => "stop".into(),
        FinishReason::Length => "length".into(),
        FinishReason::ToolCalls => "tool_calls".into(),
        FinishReason::ContentFilter => "content_filter".into(),
        FinishReason::Other(value) => value,
    });
    let mut text = String::new();
    for part in response.choice {
        match part {
            AssistantContent::Text(part) => {
                ensure!(
                    part.text.len() <= limit.saturating_sub(text.len()),
                    OutputTooLargeSnafu { limit }
                );
                text.push_str(&part.text);
            }
            AssistantContent::Reasoning(_) => {}
            _ => return UnsupportedOutputSnafu.fail(),
        }
    }
    ensure!(!text.is_empty(), EmptyTextSnafu);
    let mut usage = BTreeMap::new();
    for (counter, value) in [
        ("input_tokens", response.usage.input_tokens),
        ("output_tokens", response.usage.output_tokens),
        ("total_tokens", response.usage.total_tokens),
    ] {
        if let Some(value) = value {
            usage.insert(
                counter.into(),
                i64::try_from(value).context(UsageCountSnafu { counter })?,
            );
        }
    }
    history.push(ChatMessage {
        role: "user".into(),
        content: prompt,
    });
    history.push(ChatMessage {
        role: "assistant".into(),
        content: text.clone(),
    });
    Ok(LlmOutputs {
        text,
        history,
        usage: (!usage.is_empty()).then_some(usage),
        finish_reason,
    })
}

/// Prepares a complete terminal-generation task without acquiring execution resources.
pub fn prepare_llm(value: Value) -> Result<PreparedNode, NodeBuildError> {
    PreparedNode::typed_task(
        LlmNode {
            config: parse_config(value)?,
            client: OnceLock::new(),
        },
        NodeMetadata::default(),
    )
}

inventory::submit! { NodeRegistration { kind: KIND, factory: NodeFactory::Plain(prepare_llm) } }

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{NodeValue, ValueType};
    use serde_json::json;

    #[test]
    fn configuration_defaults_are_static_and_explicit_values_are_preserved() {
        let defaults = parse_config(json!({"model": "example"})).unwrap();
        assert_eq!(defaults.base_url, "https://api.openai.com/v1");
        assert_eq!(defaults.api_key_env, "OPENAI_API_KEY");
        assert_eq!(defaults.timeout_ms, 60_000);
        assert_eq!(defaults.max_output_bytes, 1_048_576);
        assert!(defaults.temperature.is_none());
        assert!(defaults.max_tokens.is_none());
        let config = parse_config(json!({"model": "custom", "base_url": "http://localhost:1234/prefix/", "api_key_env": "CUSTOM_KEY", "temperature": 0.0, "max_tokens": 5, "timeout_ms": 7, "max_output_bytes": 9})).unwrap();
        assert_eq!(config.base_url, "http://localhost:1234/prefix");
        assert_eq!(config.api_key_env, "CUSTOM_KEY");
        assert_eq!(config.temperature, Some(0.0));
        assert_eq!(config.max_tokens, Some(5));
        assert_eq!(config.timeout_ms, 7);
        assert_eq!(config.max_output_bytes, 9);
    }

    #[test]
    fn configuration_rejects_unknown_fields_and_invalid_values() {
        for patch in [
            json!({"model": " "}),
            json!({"stream": false}),
            json!({"api_key": "secret"}),
            json!({"provider": "unknown"}),
            json!({"base_url": "/v1"}),
            json!({"base_url": "file:///tmp/model"}),
            json!({"base_url": "https://user:secret@host/v1"}),
            json!({"base_url": "https://host/v1?key=secret"}),
            json!({"base_url": "https://host/v1#fragment"}),
            json!({"api_key_env": ""}),
            json!({"api_key_env": "bad=name"}),
            json!({"temperature": -0.1}),
            json!({"temperature": 2.1}),
            json!({"temperature": null}),
            json!({"max_tokens": 0}),
            json!({"max_tokens": -1}),
            json!({"max_tokens": 1.5}),
            json!({"max_tokens": null}),
            json!({"timeout_ms": 0}),
            json!({"timeout_ms": 1.5}),
            json!({"timeout_ms": 1e30}),
            json!({"max_output_bytes": 0}),
        ] {
            let mut config = json!({"model": "example"});
            config
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(parse_config(config).is_err(), "{patch}");
        }
        assert!(parse_config(json!({})).is_err());
        let error =
            parse_config(json!({"model": "example-model", "base_url": "/relative"})).unwrap_err();
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<LlmBuildError>()
            .unwrap();
        assert!(source.source().unwrap().is::<url::ParseError>());
    }

    #[test]
    fn fixed_ports_reflect_nested_messages_and_optional_metadata() {
        let inputs = LlmInputs::ports();
        assert_eq!(inputs.len(), 3);
        assert_eq!(inputs[0].name, "prompt");
        assert!(inputs[0].required);
        assert_eq!(
            inputs[2].value_type,
            ValueType::List(Box::new(ValueType::Object))
        );
        assert!(!inputs[2].required);
        let outputs = LlmOutputs::ports();
        assert_eq!(outputs.len(), 4);
        assert!(outputs[0].required && outputs[1].required);
        assert!(!outputs[2].required && !outputs[3].required);
        assert_eq!(
            outputs[2].value_type,
            ValueType::Map(Box::new(ValueType::Int64))
        );
        let response = LlmOutputs {
            text: "reply".into(),
            history: vec![ChatMessage {
                role: "assistant".into(),
                content: "reply".into(),
            }],
            usage: None,
            finish_reason: None,
        }
        .into_values()
        .unwrap();
        assert_eq!(
            response["history"],
            json!([{"role": "assistant", "content": "reply"}])
        );
        assert!(!response.contains_key("usage"));
        assert!(!response.contains_key("finish_reason"));
    }

    #[test]
    fn nested_history_decoding_rejects_unknown_missing_and_mistyped_fields() {
        for (history, path) in [
            (json!(null), "/history"),
            (json!([{"role": "user"}]), "/history/0/content"),
            (
                json!([{"role": "user", "content": 42}]),
                "/history/0/content",
            ),
            (
                json!([{"role": "user", "content": "hello", "extra": true}]),
                "/history/0/extra",
            ),
        ] {
            let values = mf_runtime::NodeValues::from([
                ("prompt".into(), "next".into()),
                ("history".into(), history.into()),
            ]);
            let error = LlmInputs::from_values(values).unwrap_err();
            assert_eq!(error.pointer(), path);
        }
        let empty = LlmInputs::from_values(mf_runtime::NodeValues::from([(
            "prompt".into(),
            "hello".into(),
        )]))
        .unwrap();
        assert!(empty.history.is_none());
    }

    #[test]
    fn terminal_conversion_preserves_text_order_and_sparse_metadata() {
        let response = CompletionResponse::new(
            vec![
                AssistantContent::text("first"),
                AssistantContent::reasoning("openai", "hidden"),
                AssistantContent::text("second"),
            ],
            rig_core::completion::Usage {
                output_tokens: Some(0),
                ..Default::default()
            },
            "openai",
            json!({}),
        )
        .with_finish_reason(FinishReason::Other("service_stop".into()));
        let previous = vec![ChatMessage {
            role: "user".into(),
            content: "prior".into(),
        }];
        let result = terminal_outputs(response, "next".into(), previous.clone(), 100).unwrap();
        assert_eq!(result.text, "firstsecond");
        assert_eq!(result.history[0], previous[0]);
        assert_eq!(result.history.len(), 3);
        assert_eq!(result.history[1].role, "user");
        assert_eq!(result.history[2].content, "firstsecond");
        assert_eq!(
            result.usage.unwrap(),
            BTreeMap::from([("output_tokens".into(), 0)])
        );
        assert_eq!(result.finish_reason.as_deref(), Some("service_stop"));
    }

    #[test]
    fn terminal_conversion_rejects_unrepresentable_usage_and_counts_utf8_bytes() {
        let response = CompletionResponse::new(
            vec![AssistantContent::text("reply")],
            rig_core::completion::Usage {
                input_tokens: Some(i64::MAX as u64 + 1),
                ..Default::default()
            },
            "openai",
            json!({}),
        );
        let error = terminal_outputs(response, "next".into(), vec![], 100).unwrap_err();
        assert!(matches!(error, LlmExecutionError::UsageCount { .. }));
        assert!(error.source().unwrap().is::<std::num::TryFromIntError>());
        let response = CompletionResponse::new(
            vec![AssistantContent::text("é")],
            Default::default(),
            "openai",
            json!({}),
        );
        assert!(matches!(
            terminal_outputs(response, "next".into(), vec![], 1),
            Err(LlmExecutionError::OutputTooLarge { .. })
        ));
        let response = CompletionResponse::new(vec![], Default::default(), "openai", json!({}));
        assert!(matches!(
            terminal_outputs(response, "next".into(), vec![], 100),
            Err(LlmExecutionError::EmptyText)
        ));
    }

    #[test]
    fn prepared_factory_reflects_contracts_without_execution_resources() {
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let prepared = registry.get(KIND).unwrap().instantiate(json!({"model": "example-model", "base_url": "http://127.0.0.1:9/v1", "api_key_env": "MINIFLOW_LLM_UNAVAILABLE_CREDENTIAL"})).unwrap();
        assert_eq!(prepared.metadata.ports.inputs, LlmInputs::ports());
        assert_eq!(prepared.metadata.ports.outputs, LlmOutputs::ports());
        assert!(prepared.execution.as_task_node().is_some());
        assert!(prepared.metadata.stdin.is_none());
        let node = LlmNode {
            config: parse_config(json!({"model": "example-model"})).unwrap(),
            client: OnceLock::new(),
        };
        assert!(node.client.get().is_none());
    }
}
