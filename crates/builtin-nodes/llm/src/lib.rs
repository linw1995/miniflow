use mf_runtime::{NodeBuildError, NodeValue, deserialize_config};
use serde::Deserialize;
use serde_json::Value;
use snafu::{ResultExt, Snafu, ensure};
use std::{collections::BTreeMap, error::Error};

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

/// Checks configuration without acquiring credentials, clients, or execution resources.
pub fn validate_config(value: Value) -> Result<(), NodeBuildError> {
    parse_config(value).map(|_| ())
}

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
            assert!(validate_config(config).is_err(), "{patch}");
        }
        assert!(validate_config(json!({})).is_err());
    }

    #[test]
    fn validation_needs_no_credentials_or_network_and_retains_url_sources() {
        validate_config(json!({
            "model": "example-model",
            "base_url": "http://127.0.0.1:9/v1",
            "api_key_env": "MINIFLOW_LLM_UNAVAILABLE_CREDENTIAL"
        }))
        .unwrap();
        let error = validate_config(json!({"model": "example-model", "base_url": "/relative"}))
            .unwrap_err();
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
}
