use crate::{DeepSeekExecutionError, HttpClient, InvalidInputSnafu, PreparedConfig};
use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodeRegistration, NodeValue, Nullable, PreparedNode, TypedNodeResult, TypedTaskNode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use snafu::ensure;
use std::time::{Duration, Instant};

pub const SEARCH_KIND: &str = "builtin.deepseek.search";

fn default_count() -> u64 {
    10
}

#[derive(Debug, NodeValue)]
pub struct SearchInputs {
    pub query: String,
    #[value(default = "default_count")]
    pub count: u64,
    pub ori_question: Option<String>,
    pub localization: Option<String>,
    #[value(default)]
    pub need_content: bool,
}

#[derive(Debug, NodeValue)]
pub struct SearchOutputs {
    pub results: Vec<SearchResult>,
}

#[derive(Debug, NodeValue)]
pub struct SearchResult {
    pub rerank_score: f64,
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub summary: String,
    pub content: String,
    pub site_name: Nullable<String>,
    pub site_icon: Nullable<String>,
    pub date: Nullable<String>,
    pub authority: i64,
}

#[derive(Serialize)]
struct SearchRequest<'a> {
    query: &'a str,
    count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    ori_question: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    localization: Option<&'a str>,
    need_content: bool,
}

#[derive(Deserialize)]
struct WireSearchResult {
    rerank_score: f64,
    title: String,
    url: String,
    snippet: String,
    summary: String,
    content: String,
    site_name: Option<String>,
    site_icon: Option<String>,
    date: Option<String>,
    authority: i64,
}

impl From<WireSearchResult> for SearchResult {
    fn from(result: WireSearchResult) -> Self {
        fn nullable(value: Option<String>) -> Nullable<String> {
            match value {
                Some(value) => Nullable::Value(value),
                None => Nullable::Null,
            }
        }
        Self {
            rerank_score: result.rerank_score,
            title: result.title,
            url: result.url,
            snippet: result.snippet,
            summary: result.summary,
            content: result.content,
            site_name: nullable(result.site_name),
            site_icon: nullable(result.site_icon),
            date: nullable(result.date),
            authority: result.authority,
        }
    }
}

struct SearchNode {
    config: PreparedConfig,
    client: HttpClient,
}

impl SearchNode {
    fn validate(inputs: &SearchInputs) -> Result<(), DeepSeekExecutionError> {
        ensure!(
            (1..=128).contains(&inputs.query.chars().count()) && !inputs.query.trim().is_empty(),
            InvalidInputSnafu {
                path: "/query",
                requirement: "1-128 Unicode characters and nonblank text",
            }
        );
        ensure!(
            (1..=50).contains(&inputs.count),
            InvalidInputSnafu {
                path: "/count",
                requirement: "an integer from 1 to 50",
            }
        );
        if let Some(question) = &inputs.ori_question {
            ensure!(
                (1..=128).contains(&question.chars().count()),
                InvalidInputSnafu {
                    path: "/ori_question",
                    requirement: "1-128 characters",
                }
            );
        }
        Ok(())
    }
}

impl TypedTaskNode for SearchNode {
    type Input = SearchInputs;
    type Output = SearchOutputs;

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
        let request = SearchRequest {
            query: &inputs.query,
            count: inputs.count,
            ori_question: inputs.ori_question.as_deref(),
            localization: inputs.localization.as_deref(),
            need_content: inputs.need_content,
        };
        let request = self.client.post(&config, "search", &request)?;
        let response: SearchOutputs = context.execute_future(request, Some(deadline))??;
        context.check_future_limits(Some(deadline))?;
        Ok(response.into())
    }
}

#[derive(Deserialize)]
struct WireSearchOutputs {
    results: Vec<WireSearchResult>,
}

impl<'de> Deserialize<'de> for SearchOutputs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let outputs = WireSearchOutputs::deserialize(deserializer)?;
        Ok(Self {
            results: outputs.results.into_iter().map(Into::into).collect(),
        })
    }
}

pub fn prepare_search(value: Value) -> Result<PreparedNode, NodeBuildError> {
    let config = PreparedConfig::parse(value)?;
    let metadata = NodeMetadata {
        context_references: config.references(),
        ..Default::default()
    };
    PreparedNode::typed_task(
        SearchNode {
            config,
            client: HttpClient::default(),
        },
        metadata,
    )
}

inventory::submit! {
    NodeRegistration { kind: SEARCH_KIND, factory: NodeFactory::Plain(prepare_search) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{NodeValue, ValueType};

    #[test]
    fn ports_and_response_conversion_preserve_the_search_contract() {
        let inputs = SearchInputs::ports();
        assert!(inputs[0].required);
        assert!(inputs[1..].iter().all(|port| !port.required));
        let outputs = SearchOutputs::ports();
        assert_eq!(outputs.len(), 1);
        assert_eq!(
            outputs[0].value_type,
            ValueType::List(Box::new(ValueType::Object))
        );
        let outputs: SearchOutputs = serde_json::from_value(serde_json::json!({"results":[{
            "rerank_score":2.4,"title":"Title","url":"https://example.com","snippet":"Snippet",
            "summary":"Summary","content":"Content","site_name":null,"site_icon":null,
            "date":null,"authority":3,"future_field":true
        }]}))
        .unwrap();
        let encoded = outputs.into_values().unwrap();
        assert_eq!(encoded["results"][0]["site_name"], Value::Null);
        assert!(encoded["results"][0].get("future_field").is_none());
    }

    #[test]
    fn input_validation_enforces_unicode_and_count_limits() {
        for (query, valid) in [
            ("\u{641c}\u{7d22}".to_owned(), true),
            (String::new(), false),
            (" ".into(), false),
            ("é".repeat(128), true),
            ("é".repeat(129), false),
        ] {
            let inputs = SearchInputs {
                query,
                count: 10,
                ori_question: None,
                localization: None,
                need_content: false,
            };
            assert_eq!(SearchNode::validate(&inputs).is_ok(), valid);
        }
    }
}
