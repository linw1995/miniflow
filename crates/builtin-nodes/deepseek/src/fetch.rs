use crate::{DeepSeekExecutionError, HttpClient, InvalidInputSnafu, PreparedConfig};
use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodeRegistration, NodeValue, PreparedNode, TypedNodeResult, TypedTaskNode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use snafu::{ResultExt, ensure};
use std::time::{Duration, Instant};

pub const FETCH_KIND: &str = "builtin.deepseek.fetch";

#[derive(Debug, NodeValue)]
pub struct FetchInputs {
    pub url: String,
}

#[derive(Debug, Deserialize, NodeValue)]
pub struct FetchOutputs {
    pub title: String,
    pub content: String,
}

#[derive(Serialize)]
struct FetchRequest<'a> {
    url: &'a str,
}

struct FetchNode {
    config: PreparedConfig,
    client: HttpClient,
}

impl FetchNode {
    fn validate(inputs: &FetchInputs) -> Result<(), DeepSeekExecutionError> {
        ensure!(
            inputs.url.chars().count() <= 8192,
            InvalidInputSnafu {
                path: "/url",
                requirement: "at most 8192 characters",
            }
        );
        let url = url::Url::parse(&inputs.url).context(crate::FetchUrlSnafu)?;
        ensure!(
            matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
            InvalidInputSnafu {
                path: "/url",
                requirement: "an absolute HTTP or HTTPS URL with a host",
            }
        );
        ensure!(
            url.username().is_empty() && url.password().is_none(),
            InvalidInputSnafu {
                path: "/url",
                requirement: "no embedded username or password",
            }
        );
        Ok(())
    }
}

impl TypedTaskNode for FetchNode {
    type Input = FetchInputs;
    type Output = FetchOutputs;

    fn execute(
        &self,
        inputs: Self::Input,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        Self::validate(&inputs)?;
        let origin = Instant::now();
        let config = self
            .config
            .resolve(context)
            .map_err(DeepSeekExecutionError::from)?;
        let deadline = origin
            .checked_add(Duration::from_millis(config.timeout_ms))
            .ok_or(DeepSeekExecutionError::DeadlineRange)?;
        context.check_future_limits(Some(deadline))?;
        let request = FetchRequest { url: &inputs.url };
        let request = self.client.post(&config, "fetch", &request)?;
        let outputs: FetchOutputs = context.execute_future(request, Some(deadline))??;
        context.check_future_limits(Some(deadline))?;
        Ok(outputs.into())
    }
}

pub fn prepare_fetch(value: Value) -> Result<PreparedNode, NodeBuildError> {
    let config = PreparedConfig::parse(value)?;
    let metadata = NodeMetadata {
        context_references: config.references(),
        ..Default::default()
    };
    PreparedNode::typed_task(
        FetchNode {
            config,
            client: HttpClient::default(),
        },
        metadata,
    )
}

inventory::submit! {
    NodeRegistration { kind: FETCH_KIND, factory: NodeFactory::Plain(prepare_fetch) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_urls_require_http_host_and_no_userinfo() {
        for url in [
            "https://example.com/article",
            "http://localhost:8080/path?q=one#section",
        ] {
            FetchNode::validate(&FetchInputs { url: url.into() }).unwrap();
        }
        for url in [
            "/relative",
            "file:///tmp/article",
            "https://user@example.com/article",
            "https://user:secret@example.com/article",
        ] {
            assert!(FetchNode::validate(&FetchInputs { url: url.into() }).is_err());
        }
        assert!(
            FetchNode::validate(&FetchInputs {
                url: format!("https://example.com/{}", "a".repeat(8192))
            })
            .is_err()
        );
    }
}
