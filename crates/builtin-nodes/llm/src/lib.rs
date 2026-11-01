pub mod call;
pub mod flows;
mod tools;
pub use call::{CallOutcome, ToolCall, ToolCallEvent, ToolDefinition, ToolResult};
mod streaming;
#[cfg(feature = "test-support")]
#[path = "../tests/support/mod.rs"]
pub mod test_support;
mod transport;

use mf_runtime::{
    ConfigTemplate, ContextReference, ExecutionContext, Inputs, NodeBuildError, NodeEnum,
    NodeExecutionError, NodeFactory, NodeMetadata, NodeRegistration, NodeValue, PreparedNode,
    Shared, TypedEmitter, TypedNodeResult, TypedTaskStreamNode, ValueRef, deserialize_config,
};
use rig_core::{
    completion::{CompletionRequest, CompletionResponse, FinishReason},
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

pub type MessageMetadata = BTreeMap<String, ValueRef>;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, NodeValue)]
pub struct UsageScope {
    pub node: String,
    pub index: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, NodeValue)]
pub struct UsageNode {
    pub id: String,
    pub kind: String,
    #[value(default)]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<UsageScope>,
}

/// Workflow-owned labels for accounting, independent of provider and node identity.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, NodeValue)]
pub struct UsageAttribution {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

pub const REQUEST_STARTED_EVENT: &str = "llm.request.started";
pub const FIRST_TEXT_EVENT: &str = "llm.response.first_text";
pub const REQUEST_FINISHED_EVENT: &str = "llm.request.finished";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, NodeValue)]
pub struct CallUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<UsageAttribution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    pub call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<UsageNode>,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<BTreeMap<String, i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, NodeEnum)]
#[value(tag = "role")]
pub enum ChatMessage {
    #[value(rename = "system")]
    System {
        content: String,
        metadata: Option<MessageMetadata>,
    },
    #[value(rename = "user")]
    User {
        content: String,
        metadata: Option<MessageMetadata>,
    },
    #[value(rename = "assistant")]
    Assistant {
        content: String,
        tool_calls: Option<Vec<ToolCall>>,
        metadata: Option<MessageMetadata>,
    },
    #[value(rename = "tool")]
    ToolResult {
        tool_call_id: String,
        content: String,
        metadata: Option<MessageMetadata>,
    },
}

#[derive(Clone, Debug, NodeValue)]
pub struct LlmInputs {
    pub messages: Vec<ChatMessage>,
    #[value(default)]
    pub tools: Vec<ToolDefinition>,
    #[value(default)]
    pub tool_results: Vec<ToolResult>,
}

#[derive(Clone, Debug, NodeValue)]
pub struct LlmOutputs {
    pub status: String,
    pub outcome: CallOutcome,
    pub tool_calls: Vec<ToolCall>,
    pub usages: Vec<CallUsage>,
    pub error: Option<String>,
    pub text: String,
    pub message: Option<ChatMessage>,
    pub usage: Option<BTreeMap<String, i64>>,
    pub finish_reason: Option<String>,
    pub metadata: Option<MessageMetadata>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, NodeValue)]
pub struct TextDelta {
    pub text: String,
    pub model_call_id: String,
}

#[derive(Clone, Debug, NodeValue)]
pub struct LlmEmission {
    pub delta: Option<TextDelta>,
    pub tool_call: Option<ToolCallEvent>,
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
    capture_errors: bool,
    attribution: Option<UsageAttribution>,
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
            ("capture_errors", Value::from(false)),
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
        "attribution" if value.contains_secret() => Some("public accounting labels"),
        "attribution"
            if value
                .as_object()
                .unwrap()
                .values()
                .any(|label| label.as_str().is_some_and(|label| label.trim().is_empty())) =>
        {
            Some("nonblank accounting labels")
        }
        "model" if value.contains_secret() => Some("a public model identifier"),
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
        "max_tokens" | "timeout_ms" if value.as_f64() == Some(0.0) => Some("a positive integer"),
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
    #[snafu(display("could not publish LLM stream output: {source}"))]
    Publication { source: NodeExecutionError },
    #[snafu(display("LLM response contains unsupported nontext output"))]
    UnsupportedOutput,
    #[snafu(display("LLM response requires {requirement}"))]
    InvalidResponse { requirement: &'static str },
    #[snafu(display("LLM response contains no nonempty text"))]
    EmptyText,
    #[snafu(display("LLM usage counter `{counter}` exceeds the signed integer range"))]
    UsageCount {
        counter: &'static str,
        source: std::num::TryFromIntError,
    },
    #[snafu(display("LLM timeout exceeds the monotonic clock range"))]
    DeadlineRange,
    #[snafu(display("LLM usage metadata is invalid"))]
    UsageMetadata { source: serde_json::Error },
    #[snafu(display("could not encode LLM request"))]
    EncodeRequest { source: serde_json::Error },
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

impl TypedTaskStreamNode for LlmNode {
    type Input = LlmInputs;
    type Output = LlmOutputs;
    type Emission = LlmEmission;

    fn execute(
        &self,
        inputs: Self::Input,
        context: &mut ExecutionContext,
        emitter: &mut TypedEmitter<'_, '_, Self::Emission>,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        let outputs = self.complete(inputs, context, emitter)?;
        let skipped = [
            ("usage", outputs.usage.is_none()),
            ("finish_reason", outputs.finish_reason.is_none()),
            ("metadata", outputs.metadata.is_none()),
            ("error", outputs.error.is_none()),
            ("message", outputs.message.is_none()),
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

impl LlmNode {
    fn complete(
        &self,
        inputs: LlmInputs,
        context: &ExecutionContext,
        emitter: &mut TypedEmitter<'_, '_, LlmEmission>,
    ) -> Result<LlmOutputs, LlmExecutionError> {
        let deadline_origin = Instant::now();
        let config = self.config.resolve(context)?;
        let deadline = deadline_origin
            .checked_add(Duration::from_millis(config.timeout_ms))
            .context(DeadlineRangeSnafu)?;
        context.check_future_limits(Some(deadline))?;
        let request = call::prepare_request(&inputs)?
            .temperature(config.temperature)
            .max_tokens(config.max_tokens);
        let allow_tools = !inputs.tools.is_empty();
        let (response, mut usage) = self.request(request, &config, context, emitter, deadline)?;
        let result = response.and_then(|response| {
            if usage.usage.is_none() {
                usage.usage = reported_usage(&response.usage).ok().flatten();
            }
            terminal_outputs(response, allow_tools).and_then(|result| {
                context.check_future_limits(Some(deadline))?;
                Ok(result)
            })
        });
        let mut result = match result {
            Ok(result) => result,
            Err(error)
                if config.capture_errors
                    && !matches!(error, LlmExecutionError::Publication { .. }) =>
            {
                context.check_future_limits(None)?;
                let error = error.to_string();
                usage.error = Some(error.clone());
                LlmOutputs {
                    status: "failed".into(),
                    outcome: CallOutcome::Failed,
                    tool_calls: Vec::new(),
                    text: String::new(),
                    message: None,
                    usage: usage.usage.clone(),
                    finish_reason: None,
                    metadata: None,
                    usages: Vec::new(),
                    error: Some(error),
                }
            }
            Err(error) => return Err(error),
        };
        usage.usage = result.usage.clone();
        result.usages.push(usage);
        Ok(result)
    }
    fn request(
        &self,
        request: CompletionRequest,
        config: &Config,
        context: &ExecutionContext,
        emitter: &mut TypedEmitter<'_, '_, LlmEmission>,
        deadline: Instant,
    ) -> Result<(Result<CompletionResponse, LlmExecutionError>, CallUsage), LlmExecutionError> {
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
        let mut usage = CallUsage {
            attribution: config.attribution.clone(),
            started_at: None,
            call_id: uuid::Uuid::new_v4().to_string(),
            node: context.current_node().map(|id| UsageNode {
                id: id.into(),
                kind: KIND.into(),
                path: context
                    .invocation_path()
                    .into_iter()
                    .map(|scope| UsageScope {
                        node: scope.node_id().to_owned(),
                        index: scope.index().get(),
                    })
                    .collect(),
            }),
            model: config.model.clone(),
            usage: None,
            error: None,
        };
        usage.started_at =
            mf_telemetry::events::record(REQUEST_STARTED_EVENT, serde_json::json!(&usage))
                .and_then(|timestamp| i64::try_from(timestamp / 1_000_000).ok());
        let allow_tools = !request.tools.is_empty();
        let response = if emitter.is_connected() {
            streaming::complete(
                model.stream(request),
                emitter,
                &usage.call_id,
                deadline,
                context,
                allow_tools,
            )
        } else {
            context
                .execute_future(model.call(request), Some(deadline))
                .map_err(LlmExecutionError::from)
                .and_then(|response| {
                    response.with_context(|source| ProviderSnafu {
                        kind: source.kind().code(),
                        status: source
                            .provider_response_status()
                            .map(|status| status.as_u16()),
                    })
                })
        };
        if let Some(reported) = transport.usage() {
            usage.usage = reported_usage(&reported).ok().flatten();
        }
        if usage.usage.is_none()
            && let Ok(response) = &response
        {
            usage.usage = reported_usage(&response.usage).ok().flatten();
        }
        let mut finished = usage.clone();
        finished.error = response.as_ref().err().map(ToString::to_string);
        mf_telemetry::events::record(REQUEST_FINISHED_EVENT, serde_json::json!(finished));
        let response = response.map(|mut response| {
            if let Some(reported) = transport.usage() {
                response.usage = reported;
            }
            response
        });
        Ok((response, usage))
    }
}

fn reported_usage(
    reported: &rig_core::completion::Usage,
) -> Result<Option<BTreeMap<String, i64>>, LlmExecutionError> {
    let mut usage = BTreeMap::new();
    for (counter, value) in [
        ("input_tokens", reported.input_tokens),
        ("output_tokens", reported.output_tokens),
        ("total_tokens", reported.total_tokens),
    ] {
        if let Some(value) = value {
            usage.insert(
                counter.into(),
                i64::try_from(value).context(UsageCountSnafu { counter })?,
            );
        }
    }
    Ok((!usage.is_empty()).then_some(usage))
}

fn finish_reason(response: &CompletionResponse) -> Option<String> {
    response.finish_reason().map(|reason| match reason {
        FinishReason::Stop => "stop".into(),
        FinishReason::Length => "length".into(),
        FinishReason::ToolCalls => "tool_calls".into(),
        FinishReason::ContentFilter => "content_filter".into(),
        FinishReason::Other(value) => value,
    })
}

fn terminal_outputs(
    response: CompletionResponse,
    allow_tools: bool,
) -> Result<LlmOutputs, LlmExecutionError> {
    let usage = reported_usage(&response.usage)?;
    let result = call::terminal_result(response)?;
    if !allow_tools {
        ensure!(result.tool_calls.is_empty(), UnsupportedOutputSnafu);
        ensure!(!result.text.is_empty(), EmptyTextSnafu);
    }
    let message = (!result.text.is_empty() || !result.tool_calls.is_empty())
        .then(|| call::assistant_message(&result));
    Ok(LlmOutputs {
        status: "succeeded".into(),
        outcome: result.outcome,
        tool_calls: result.tool_calls,
        usages: Vec::new(),
        error: None,
        metadata: None,
        text: result.text,
        message,
        usage,
        finish_reason: result.finish_reason,
    })
}

/// Prepares text/tool streams and their authoritative result without acquiring resources.
pub fn prepare_llm(value: Value) -> Result<PreparedNode, NodeBuildError> {
    let config = PreparedConfig::parse(value)?;
    let metadata = NodeMetadata {
        context_references: config.references(),
        ..Default::default()
    };
    PreparedNode::typed_task_stream(
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
    use mf_runtime::{InputValue, NodeValue, OutputValue, ValueType};
    use rig_core::completion::AssistantContent;
    use serde_json::json;

    #[test]
    fn configuration_defaults_are_static_and_explicit_values_are_preserved() {
        let defaults = parse_config(json!({"model": "example"})).unwrap();
        assert_eq!(defaults.base_url, "https://api.openai.com/v1");
        assert_eq!(defaults.api_key_env, "OPENAI_API_KEY");
        assert_eq!(defaults.timeout_ms, 60_000);
        assert!(defaults.temperature.is_none());
        assert!(defaults.max_tokens.is_none());
        let config = parse_config(json!({"model": "custom", "base_url": "http://localhost:1234/prefix/", "api_key_env": "CUSTOM_KEY", "temperature": 0.0, "max_tokens": 5, "timeout_ms": 7})).unwrap();
        assert_eq!(config.base_url, "http://localhost:1234/prefix");
        assert_eq!(config.api_key_env, "CUSTOM_KEY");
        assert_eq!(config.temperature, Some(0.0));
        assert_eq!(config.max_tokens, Some(5));
        assert_eq!(config.timeout_ms, 7);
    }

    #[test]
    fn configuration_rejects_unknown_fields_and_invalid_values() {
        for patch in [
            json!({"model": " "}),
            json!({"attribution": {"purpose": " "}}),
            json!({"attribution": {"profile": ""}}),
            json!({"attribution": {"purpose": 42}}),
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
            json!({"max_output_bytes": 1024}),
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
        ] {
            config[field] = json!({"$ref": format!("params.{field}")});
        }
        validate_config(config.clone()).unwrap();
        let prepared = prepare_llm(config.clone()).unwrap();
        assert_eq!(prepared.metadata.context_references.len(), 6);
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
        assert_eq!(inputs[0].name, "messages");
        assert!(inputs[0].required);
        assert_eq!(
            inputs[0].value_type,
            ValueType::List(Box::new(ValueType::Object))
        );
        for (port, name) in inputs[1..].iter().zip(["tools", "tool_results"]) {
            assert_eq!(port.name, name);
            assert!(!port.required);
            assert_eq!(
                port.value_type,
                ValueType::List(Box::new(ValueType::Object))
            );
        }
        let outputs = LlmOutputs::ports();
        for name in ["status", "usages", "text", "tool_calls", "outcome"] {
            assert!(
                outputs
                    .iter()
                    .any(|port| port.name == name && port.required)
            );
        }
        for name in ["message", "usage", "finish_reason", "metadata", "error"] {
            assert!(
                outputs
                    .iter()
                    .any(|port| port.name == name && !port.required)
            );
        }
        let response = LlmOutputs {
            status: "succeeded".into(),
            outcome: CallOutcome::Completed,
            tool_calls: Vec::new(),
            usages: Vec::new(),
            error: None,
            metadata: None,
            text: "reply".into(),
            message: Some(ChatMessage::Assistant {
                metadata: None,
                tool_calls: None,
                content: "reply".into(),
            }),
            usage: None,
            finish_reason: None,
        }
        .into_values()
        .unwrap();
        assert_eq!(
            response["message"],
            json!({"role": "assistant", "content": "reply"})
        );
        assert!(!response.contains_key("usage"));
        assert!(!response.contains_key("finish_reason"));
    }

    #[test]
    fn message_variants_preserve_the_role_tagged_json_contract() {
        for expected in [
            json!({"role":"system","content":"Be brief"}),
            json!({"role":"user","content":"Look up both"}),
            json!({"role":"assistant","content":"","tool_calls":[
                {"id":"one","name":"lookup","arguments":{"id":1}},
                {"id":"two","name":"lookup","arguments":{"id":2}}
            ]}),
            json!({"role":"tool","tool_call_id":"two","content":"result","metadata":{"source":{"ids":[1,2]}}}),
        ] {
            let message = ChatMessage::decode(expected.clone().into()).unwrap();
            assert_eq!(message.encode().unwrap(), expected);
        }
    }

    #[test]
    fn nested_messages_decoding_rejects_unknown_missing_and_mistyped_fields() {
        for (messages, path) in [
            (json!(null), "/messages"),
            (json!([{"content":"hello"}]), "/messages/0/role"),
            (
                json!([{"role":"unknown","content":"hello"}]),
                "/messages/0/role",
            ),
            (
                json!([{"role":"system","content":"hello","tool_calls":[]}]),
                "/messages/0/tool_calls",
            ),
            (
                json!([{"role":"user","content":"hello","tool_call_id":"one"}]),
                "/messages/0/tool_call_id",
            ),
            (
                json!([{"role":"assistant","content":"hello","tool_call_id":"one"}]),
                "/messages/0/tool_call_id",
            ),
            (
                json!([{"role":"tool","content":"result"}]),
                "/messages/0/tool_call_id",
            ),
            (
                json!([{"role":"tool","tool_call_id":"one","content":"result","tool_calls":[]}]),
                "/messages/0/tool_calls",
            ),
            (json!([{"role": "user"}]), "/messages/0/content"),
            (
                json!([{"role":"user","content":"hello","metadata":null}]),
                "/messages/0/metadata",
            ),
            (
                json!([{"role":"user","content":"hello","metadata":[]}]),
                "/messages/0/metadata",
            ),
            (
                json!([{"role": "user", "content": 42}]),
                "/messages/0/content",
            ),
            (
                json!([{"role": "user", "content": "hello", "extra": true}]),
                "/messages/0/extra",
            ),
        ] {
            let values = mf_runtime::NodeValues::from([("messages".into(), messages.into())]);
            let error = LlmInputs::from_values(values).unwrap_err();
            assert_eq!(error.pointer(), path);
        }
        let input = LlmInputs::from_values(mf_runtime::NodeValues::from([(
            "messages".into(),
            json!([{"role":"user","content":"hello"}]).into(),
        )]))
        .unwrap();
        assert!(input.tools.is_empty());
        assert!(input.tool_results.is_empty());
        assert!(LlmInputs::from_values(Default::default()).is_err());
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
        let result = terminal_outputs(response, false).unwrap();
        assert_eq!(result.text, "firstsecond");
        assert_eq!(
            result.message,
            Some(ChatMessage::Assistant {
                content: "firstsecond".into(),
                tool_calls: None,
                metadata: None,
            })
        );
        assert_eq!(
            result.usage.unwrap(),
            BTreeMap::from([("output_tokens".into(), 0)])
        );
        assert_eq!(result.finish_reason.as_deref(), Some("service_stop"));
    }

    #[test]
    fn terminal_conversion_preserves_utf8_and_rejects_invalid_counts_or_empty_text() {
        let response = CompletionResponse::new(
            vec![AssistantContent::text("reply")],
            rig_core::completion::Usage {
                input_tokens: Some(i64::MAX as u64 + 1),
                ..Default::default()
            },
            "openai",
            json!({}),
        );
        let error = terminal_outputs(response, false).unwrap_err();
        assert!(matches!(error, LlmExecutionError::UsageCount { .. }));
        assert!(error.source().unwrap().is::<std::num::TryFromIntError>());
        let response = CompletionResponse::new(
            vec![AssistantContent::text("é")],
            Default::default(),
            "openai",
            json!({}),
        );
        assert_eq!(terminal_outputs(response, false).unwrap().text, "é");
        let response = CompletionResponse::new(vec![], Default::default(), "openai", json!({}));
        assert!(matches!(
            terminal_outputs(response, false),
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
