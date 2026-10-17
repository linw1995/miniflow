#[cfg(feature = "test-support")]
#[path = "../tests/support/mod.rs"]
pub mod test_support;
mod transport;

use mf_runtime::{
    ConfigTemplate, ContextReference, ExecutionContext, Inputs, NodeBuildError, NodeExecutionError,
    NodeFactory, NodeMetadata, NodeRegistration, NodeValue, PreparedNode, Shared, TypedNodeResult,
    TypedTaskNode, ValueRef, deserialize_config,
};
use rig_core::{
    completion::{AssistantContent, CompletionRequest, CompletionResponse, FinishReason, Message},
    error::ProviderError,
    providers::openai::OpenAIConfig,
};
use rig_reqwest::ReqwestClient;
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

#[derive(mf_runtime::NodeInputs)]
struct Config {
    model: String,
    base_url: String,
    api_key_env: String,
    api_key: Option<Shared<String>>,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    timeout_ms: u64,
    max_output_bytes: usize,
}

#[derive(Debug, Snafu)]
pub enum LlmConfigError {
    #[snafu(display("configuration `{field}` requires {requirement}"))]
    InvalidConfig {
        field: String,
        requirement: &'static str,
    },
    #[snafu(display("configuration `base_url` is not a valid absolute URL: {source}"))]
    BaseUrl { source: url::ParseError },
    #[snafu(display("invalid LLM configuration: {source}"))]
    Decode {
        source: mf_runtime::InputDecodeError,
    },
    #[snafu(display("configuration `{field}`: {source}"))]
    Template {
        field: String,
        source: mf_runtime::ConfigTemplateError,
    },
    #[snafu(display("configuration `{field}` has an incompatible type: {source}"))]
    ConfigType {
        field: String,
        source: mf_runtime::TypeMismatch,
    },
}

pub type LlmBuildError = LlmConfigError;

impl From<LlmConfigError> for NodeBuildError {
    fn from(source: LlmConfigError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

struct PreparedConfig {
    fields: BTreeMap<String, ConfigTemplate>,
}

impl PreparedConfig {
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
            ("timeout_ms", Value::from(60_000_u64)),
            ("max_output_bytes", Value::from(1_048_576_u64)),
        ] {
            fields.entry(name.into()).or_insert(value);
        }
        let ports = <Config as mf_runtime::NodeInputs>::ports();
        let mut templates = BTreeMap::new();
        for (field, value) in fields {
            let port =
                ports
                    .iter()
                    .find(|port| port.name == field)
                    .context(InvalidConfigSnafu {
                        field: &field,
                        requirement: "a supported LLM configuration field",
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

    fn resolve(&self, context: &ExecutionContext) -> Result<Config, LlmConfigError> {
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
        // Decode before semantic validation so dynamic values never reach unchecked accessors.
        let mut config =
            <Config as mf_runtime::NodeInputs>::from_inputs(fields.clone()).context(DecodeSnafu)?;
        for (field, value) in &fields {
            validate_field(field, value)?;
        }
        config.base_url = url::Url::parse(&config.base_url)
            .context(BaseUrlSnafu)?
            .as_str()
            .trim_end_matches('/')
            .to_owned();
        Ok(config)
    }
}

fn validate_field(field: &str, value: &ValueRef) -> Result<(), LlmConfigError> {
    let requirement = match field {
        "model" if value.as_str().unwrap().trim().is_empty() => Some("a nonblank identifier"),
        "api_key" if value.as_str().unwrap().is_empty() => Some("a nonempty credential"),
        "api_key_env"
            if value.as_str().unwrap().trim().is_empty()
                || value.as_str().unwrap().contains(['=', '\0']) =>
        {
            Some("a nonblank valid environment-variable name")
        }
        "temperature"
            if !value
                .as_f64()
                .is_some_and(|value| value.is_finite() && (0.0..=2.0).contains(&value)) =>
        {
            Some("a finite number from 0 to 2")
        }
        "max_tokens" | "timeout_ms" | "max_output_bytes" if value.as_f64() == Some(0.0) => {
            Some("a positive integer")
        }
        "base_url" => {
            let base = url::Url::parse(value.as_str().unwrap()).context(BaseUrlSnafu)?;
            ensure!(
                matches!(base.scheme(), "http" | "https") && base.host_str().is_some(),
                InvalidConfigSnafu {
                    field,
                    requirement: "an absolute HTTP or HTTPS URL",
                }
            );
            ensure!(
                base.username().is_empty()
                    && base.password().is_none()
                    && base.query().is_none()
                    && base.fragment().is_none(),
                InvalidConfigSnafu {
                    field,
                    requirement: "no embedded credentials, query, or fragment",
                }
            );
            None
        }
        _ => None,
    };
    if let Some(requirement) = requirement {
        return InvalidConfigSnafu { field, requirement }.fail();
    }
    Ok(())
}

/// Validates literals and template syntax without resolving runtime values.
pub fn validate_config(value: Value) -> Result<(), NodeBuildError> {
    PreparedConfig::parse(value).map(|_| ())
}

#[cfg(test)]
fn parse_config(value: Value) -> Result<Config, NodeBuildError> {
    Ok(PreparedConfig::parse(value)?.resolve(&ExecutionContext::default())?)
}

#[derive(Snafu)]
pub enum LlmExecutionError {
    #[snafu(transparent)]
    Configuration { source: LlmConfigError },
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
    config: PreparedConfig,
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
        let started = Instant::now();
        let config = self.config.resolve(context)?;
        let deadline = started
            .checked_add(Duration::from_millis(config.timeout_ms))
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
        let key = if let Some(key) = &config.api_key {
            key.value()
                .as_str()
                .expect("validated credential")
                .to_owned()
        } else {
            let key = std::env::var(&config.api_key_env).context(CredentialsSnafu {
                variable: &config.api_key_env,
            })?;
            ensure!(
                !key.is_empty(),
                EmptyCredentialsSnafu {
                    variable: &config.api_key_env
                }
            );
            key
        };
        let transport = transport::TerminalTransport::new(self.client()?);
        let model = OpenAIConfig::new(key)
            .with_base_url(&config.base_url)
            .connect(transport.clone())
            .chat(&config.model);
        let messages = history.iter().map(|message| {
            if message.role == "user" {
                Message::user(&message.content)
            } else {
                Message::assistant(&message.content)
            }
        });
        let mut request = CompletionRequest::new(&inputs.prompt)
            .messages(messages)
            .temperature(config.temperature)
            .max_tokens(config.max_tokens);
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
        let result = terminal_outputs(response, inputs.prompt, history, config.max_output_bytes)?;
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
    let config = PreparedConfig::parse(value)?;
    let metadata = NodeMetadata {
        context_references: config.references(),
        ..Default::default()
    };
    PreparedNode::typed_task(
        LlmNode {
            config,
            client: OnceLock::new(),
        },
        metadata,
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
        let error = parse_config(json!({"model": "example-model", "base_url": "/relative"}))
            .err()
            .expect("relative URLs must be rejected");
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<LlmBuildError>()
            .unwrap();
        assert!(source.source().unwrap().is::<url::ParseError>());
    }

    #[test]
    fn templates_declare_dependencies_without_resolving_configuration() {
        let mut config = json!({});
        for field in [
            "model",
            "base_url",
            "api_key",
            "temperature",
            "max_tokens",
            "timeout_ms",
            "max_output_bytes",
        ] {
            config[field] = json!({"$ref": format!("params.{field}")});
        }
        validate_config(config.clone()).unwrap();
        let prepared = prepare_llm(config.clone()).unwrap();
        assert_eq!(prepared.metadata.context_references.len(), 7);
        assert_eq!(prepared.metadata.ports.inputs, LlmInputs::ports());
        assert_eq!(prepared.metadata.ports.outputs, LlmOutputs::ports());
        config["api_key_env"] = "OTHER_KEY".into();
        assert!(validate_config(config).is_err());
        for patch in [
            json!({"api_key":{"$template":"literal-key"}}),
            json!({"api_key_env":{"$ref":"params.name"}}),
            json!({"model":{"$ref":"params.model", "unexpected":true}}),
        ] {
            let mut config = json!({"model":"example"});
            config
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(validate_config(config).is_err());
        }
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
            config: PreparedConfig::parse(json!({"model": "example-model"})).unwrap(),
            client: OnceLock::new(),
        };
        assert!(node.client.get().is_none());
    }
}
