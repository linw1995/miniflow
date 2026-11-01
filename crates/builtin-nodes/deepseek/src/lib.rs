mod fetch;
mod search;
#[cfg(feature = "test-support")]
#[path = "../tests/support/mod.rs"]
pub mod test_support;
mod transport;

pub use fetch::{FETCH_KIND, FetchInputs, FetchOutputs, prepare_fetch};
pub use search::{SEARCH_KIND, SearchInputs, SearchOutputs, SearchResult, prepare_search};

use mf_runtime::{
    ConfigTemplate, ContextReference, ExecutionContext, Inputs, NodeBuildError, NodeExecutionError,
    Shared, deserialize_config,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    sync::{Arc, OnceLock},
};

#[derive(mf_runtime::NodeInputs)]
struct Config {
    base_url: String,
    api_key_env: String,
    api_key: Option<Shared<String>>,
    timeout_ms: u64,
}

#[derive(Debug, Snafu)]
pub enum DeepSeekConfigError {
    #[snafu(display("configuration `{field}` requires {requirement}"))]
    InvalidConfig {
        field: String,
        requirement: &'static str,
    },
    #[snafu(display("configuration `base_url` is not a valid absolute URL: {source}"))]
    BaseUrl { source: url::ParseError },
    #[snafu(display("invalid DeepSeek configuration: {source}"))]
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

impl From<DeepSeekConfigError> for NodeBuildError {
    fn from(source: DeepSeekConfigError) -> Self {
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
            !(fields.contains_key("api_key") && fields.contains_key("api_key_env")),
            InvalidConfigSnafu {
                field: "api_key",
                requirement: "exactly one credential source",
            }
        );
        for (name, value) in [
            ("base_url", Value::from("https://api.deepseek.com")),
            ("api_key_env", Value::from("DEEPSEEK_API_KEY")),
            ("timeout_ms", Value::from(60_000_u64)),
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
                        requirement: "a supported DeepSeek configuration field",
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

    fn resolve(&self, context: &ExecutionContext) -> Result<Config, DeepSeekConfigError> {
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

fn validate_field(field: &str, value: &mf_runtime::ValueRef) -> Result<(), DeepSeekConfigError> {
    let requirement = match field {
        "api_key_env"
            if value.as_str().unwrap().trim().is_empty()
                || value.as_str().unwrap().contains(['=', '\0']) =>
        {
            Some("a nonblank valid environment-variable name")
        }
        "timeout_ms" if value.as_u64() == Some(0) => Some("a positive integer"),
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

#[derive(Snafu)]
pub enum DeepSeekExecutionError {
    #[snafu(transparent)]
    Configuration { source: DeepSeekConfigError },
    #[snafu(display("DeepSeek input `{path}` requires {requirement}"))]
    InvalidInput {
        path: String,
        requirement: &'static str,
    },
    #[snafu(display("DeepSeek Fetch input `url` is not a valid absolute URL: {source}"))]
    FetchUrl { source: url::ParseError },
    #[snafu(display("could not read DeepSeek credential variable `{variable}`"))]
    Credentials {
        variable: String,
        source: std::env::VarError,
    },
    #[snafu(display("DeepSeek credential variable `{variable}` is empty"))]
    EmptyCredentials { variable: String },
    #[snafu(display("could not initialize the DeepSeek HTTP client"))]
    ClientInitialization { source: Arc<reqwest::Error> },
    #[snafu(display("could not initialize the DeepSeek HTTP reactor"))]
    RuntimeInitialization { source: Arc<std::io::Error> },
    #[snafu(display("DeepSeek {operation} request failed: {source}"))]
    Request {
        operation: &'static str,
        source: reqwest::Error,
    },
    #[snafu(display("DeepSeek {operation} request failed with HTTP {status}"))]
    ProviderStatus {
        operation: &'static str,
        status: reqwest::StatusCode,
    },
    #[snafu(display("could not decode DeepSeek {operation} response: {source}"))]
    DecodeResponse {
        operation: &'static str,
        source: serde_json::Error,
    },
    #[snafu(display("DeepSeek timeout exceeds the monotonic clock range"))]
    DeadlineRange,
    #[snafu(transparent)]
    Future {
        source: mf_runtime::FutureExecutionError,
    },
}

impl fmt::Debug for DeepSeekExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeepSeekExecutionError")
            .field("message", &format_args!("{self}"))
            .finish()
    }
}

impl From<DeepSeekExecutionError> for NodeExecutionError {
    fn from(source: DeepSeekExecutionError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

#[derive(Default)]
struct HttpClient {
    inner: OnceLock<Result<reqwest::Client, Arc<reqwest::Error>>>,
}

impl HttpClient {
    fn get(&self) -> Result<reqwest::Client, DeepSeekExecutionError> {
        self.inner
            .get_or_init(|| {
                reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .retry(reqwest::retry::never())
                    .pool_max_idle_per_host(0)
                    .build()
                    .map_err(Arc::new)
            })
            .as_ref()
            .cloned()
            .map_err(Arc::clone)
            .context(ClientInitializationSnafu)
    }

    fn post<Request, Response>(
        &self,
        config: &Config,
        operation: &'static str,
        body: &Request,
    ) -> Result<
        impl std::future::Future<Output = Result<Response, DeepSeekExecutionError>>,
        DeepSeekExecutionError,
    >
    where
        Request: Serialize + Sync,
        Response: DeserializeOwned,
    {
        transport::bind(async move {
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
            let response = {
                let request = self
                    .get()?
                    .post(format!(
                        "{}/v1/tools/{operation}",
                        config.base_url.trim_end_matches('/')
                    ))
                    .json(body);
                let request = if key.is_empty() {
                    request
                } else {
                    request.bearer_auth(key)
                };
                request.send().await.context(RequestSnafu { operation })?
            };
            let status = response.status();
            ensure!(
                status.is_success(),
                ProviderStatusSnafu { operation, status }
            );
            let body = response.bytes().await.context(RequestSnafu { operation })?;
            serde_json::from_slice(&body).context(DecodeResponseSnafu { operation })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse_config(value: Value) -> Result<Config, NodeBuildError> {
        Ok(PreparedConfig::parse(value)?.resolve(&ExecutionContext::default())?)
    }

    #[test]
    fn configuration_defaults_and_explicit_values_are_preserved() {
        let defaults = parse_config(json!({})).unwrap();
        assert_eq!(defaults.base_url, "https://api.deepseek.com");
        assert_eq!(defaults.api_key_env, "DEEPSEEK_API_KEY");
        assert_eq!(defaults.timeout_ms, 60_000);
        let config = parse_config(json!({
            "base_url": "http://localhost:1234/prefix/",
            "api_key_env": "CUSTOM_KEY",
            "timeout_ms": 7
        }))
        .unwrap();
        assert_eq!(config.base_url, "http://localhost:1234/prefix");
        assert_eq!(config.api_key_env, "CUSTOM_KEY");
        assert_eq!(config.timeout_ms, 7);
    }

    #[test]
    fn configuration_rejects_unknown_fields_and_invalid_values() {
        for patch in [
            json!({"base_url": "/v1"}),
            json!({"base_url": "file:///tmp/api"}),
            json!({"base_url": "https://user:secret@host"}),
            json!({"base_url": "https://host?key=secret"}),
            json!({"base_url": "https://host#fragment"}),
            json!({"api_key": "secret"}),
            json!({"api_key_env": ""}),
            json!({"api_key_env": "bad=name"}),
            json!({"timeout_ms": 0}),
            json!({"timeout_ms": 1.5}),
            json!({"timeout_ms": null}),
            json!({"capture_errors": true}),
        ] {
            assert!(parse_config(patch).is_err());
        }
        assert!(
            parse_config(json!({"api_key_env":"ONE", "api_key":{"$ref":"params.key"}})).is_err()
        );
    }

    #[test]
    fn templates_declare_dependencies_without_resolving_configuration() {
        let config = json!({
            "base_url": {"$ref":"params.base_url"},
            "api_key": {"$ref":"params.api_key"},
            "timeout_ms": {"$ref":"params.timeout_ms"}
        });
        let prepared = PreparedConfig::parse(config).unwrap();
        assert_eq!(prepared.references().len(), 3);
        for invalid in [
            json!({"api_key":{"$template":"literal-key"}}),
            json!({"api_key_env":{"$ref":"params.name"}}),
        ] {
            assert!(PreparedConfig::parse(invalid).is_err());
        }
    }
}
