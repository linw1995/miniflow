mod support;

use mf_runtime::{
    ExecutionContext, FlowNode, Inputs, NodeMetadata, NodePorts, NodeRegistry, NodeResult,
    NodeValue, PortSpec, StreamCancellation, ValueType,
};
use mfn_llm::{
    LlmExecutionError, MODEL_INFO_KIND, ModelInfoOutputs, ModelInfoQuery, ModelListQuery,
    list_models, query_model_info,
};
use serde_json::json;
use std::{error::Error, time::Duration};
use support::{Reply, Server};

fn context_with_key(api_key: &str) -> ExecutionContext {
    let source: FlowNode<()> = FlowNode {
        definition_id: "params".into(),
        node: (),
        metadata: NodeMetadata {
            ports: NodePorts {
                inputs: vec![],
                outputs: vec![PortSpec::new("api_key", ValueType::String, true)],
            },
            ..Default::default()
        },
        output_keys: std::sync::OnceLock::new(),
    };
    ExecutionContext::for_message(
        &source,
        NodeResult {
            outputs: [("api_key".into(), api_key.into())].into(),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn query_model_info_normalizes_context_window_and_handles_keyless_requests() {
    let server = Server::start_models(vec![
        Reply::models(json!([
            {
                "id": "openrouter/claude-sonnet",
                "name": "Claude Sonnet",
                "owned_by": "anthropic",
                "created": 1700000000,
                "context_length": 200000,
                "top_provider": {"max_completion_tokens": 16384}
            }
        ])),
        Reply::models(json!([
            {
                "id": "gpt-4o",
                "owned_by": "system"
            }
        ])),
    ]);
    let context = ExecutionContext::default();

    let mut query = ModelInfoQuery::new("openrouter/claude-sonnet", &server.base_url);
    query.api_key = Some("synthetic-key".into());
    let info = query_model_info(&query, &context).unwrap().unwrap();
    assert_eq!(info.id, "openrouter/claude-sonnet");
    assert_eq!(info.name.as_deref(), Some("Claude Sonnet"));
    assert_eq!(info.owned_by.as_deref(), Some("anthropic"));
    assert_eq!(info.created_at, Some(1_700_000_000));
    assert_eq!(info.context_window, Some(200_000));
    assert_eq!(info.max_output_tokens, Some(16_384));

    let keyless = ModelInfoQuery::new("gpt-4o", &server.base_url);
    let minimal = query_model_info(&keyless, &context).unwrap().unwrap();
    assert_eq!(minimal.id, "gpt-4o");
    assert_eq!(minimal.context_window, None);
    assert_eq!(minimal.max_output_tokens, None);

    let requests = server.model_requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].headers.starts_with("GET /prefix/v1/models "));
    assert!(
        requests[0]
            .headers
            .to_ascii_lowercase()
            .contains("authorization: bearer synthetic-key")
    );
    assert!(
        !requests[1]
            .headers
            .to_ascii_lowercase()
            .contains("authorization:")
    );
    assert!(server.requests().is_empty());
}

#[test]
fn query_model_info_does_not_follow_redirects_and_hides_provider_error_bodies() {
    let target = Server::start_models(vec![Reply::models(json!([
        {"id": "example-model", "context_window": 64000}
    ]))]);
    let mut redirect = Reply::json(json!({"error": {"message": "secret-redirect-body"}}));
    redirect.status = 302;
    redirect
        .headers
        .push(("Location".into(), format!("{}/models", target.base_url)));
    let mut failure = Reply::json(json!({"error": {"message": "private-upstream-secret"}}));
    failure.status = 403;
    let server = Server::start_models(vec![redirect, failure]);
    let context = ExecutionContext::default();

    let query = ModelInfoQuery::new("example-model", &server.base_url);
    let redirect_error = query_model_info(&query, &context).unwrap_err();
    assert!(matches!(
        redirect_error,
        LlmExecutionError::Provider {
            status: Some(302),
            ..
        }
    ));
    assert!(!redirect_error.to_string().contains("secret-redirect-body"));
    assert!(
        !format!("{redirect_error:?}").contains("secret-redirect-body"),
        "{redirect_error:?}"
    );
    assert!(target.model_requests().is_empty());
    assert!(target.requests().is_empty());

    let forbidden_error = query_model_info(&query, &context).unwrap_err();
    assert!(matches!(
        forbidden_error,
        LlmExecutionError::Provider {
            status: Some(403),
            ..
        }
    ));
    assert!(
        !forbidden_error
            .to_string()
            .contains("private-upstream-secret")
    );
    assert!(!format!("{forbidden_error:?}").contains("private-upstream-secret"));
    let source = forbidden_error.source().unwrap();
    assert!(source.is::<Box<rig_core::error::ProviderError>>());
    assert_eq!(server.model_requests().len(), 2);
}

#[test]
fn model_info_node_publishes_found_missing_and_captured_failure_outputs() {
    let mut forbidden = Reply::json(json!({"error": {"message": "hidden-provider-detail"}}));
    forbidden.status = 403;
    let server = Server::start_models(vec![
        Reply::models(json!([
            {"id": "vllm-model", "max_model_len": 32768, "max_output_tokens": 4096}
        ])),
        Reply::models(json!([
            {"id": "other-model", "context_window": 16384}
        ])),
        forbidden,
    ]);
    let registry = NodeRegistry::from_inventory().unwrap();
    let factory = registry.get(MODEL_INFO_KIND).unwrap();

    // Found with limits.
    let prepared = factory
        .instantiate(json!({
            "model": "vllm-model",
            "base_url": server.base_url,
            "api_key": {"$ref": "params.api_key"}
        }))
        .unwrap();
    let mut context = context_with_key("node-key");
    let result = prepared
        .execution
        .as_task_node()
        .unwrap()
        .execute(Inputs::new(), &mut context)
        .unwrap();
    assert!(result.skipped.contains("error"));
    assert!(!result.skipped.contains("context_window"));
    assert!(!result.skipped.contains("max_output_tokens"));
    let outputs = ModelInfoOutputs::from_values(result.outputs).unwrap();
    assert_eq!(outputs.status, "succeeded");
    assert!(outputs.found);
    assert_eq!(outputs.model, "vllm-model");
    assert_eq!(outputs.context_window, Some(32_768));
    assert_eq!(outputs.max_output_tokens, Some(4_096));
    assert_eq!(outputs.info.context_window, Some(32_768));

    // Model absent from listing.
    let mut context = context_with_key("");
    let result = prepared
        .execution
        .as_task_node()
        .unwrap()
        .execute(Inputs::new(), &mut context)
        .unwrap();
    assert!(result.skipped.contains("context_window"));
    assert!(result.skipped.contains("max_output_tokens"));
    assert!(result.skipped.contains("error"));
    assert!(!result.skipped.contains("info"));
    let outputs = ModelInfoOutputs::from_values(result.outputs).unwrap();
    assert_eq!(outputs.status, "not_found");
    assert!(!outputs.found);
    assert_eq!(outputs.info.id, "vllm-model");
    assert_eq!(outputs.info.context_window, None);

    // Captured failure preserves non-skipped info object and hides provider body.
    let captured = factory
        .instantiate(json!({
            "model": "vllm-model",
            "base_url": server.base_url,
            "api_key": {"$ref": "params.api_key"},
            "capture_errors": true
        }))
        .unwrap();
    let mut context = context_with_key("node-key");
    let result = captured
        .execution
        .as_task_node()
        .unwrap()
        .execute(Inputs::new(), &mut context)
        .unwrap();
    assert!(result.skipped.contains("context_window"));
    assert!(result.skipped.contains("max_output_tokens"));
    assert!(!result.skipped.contains("info"));
    assert!(!result.skipped.contains("error"));
    let outputs = ModelInfoOutputs::from_values(result.outputs).unwrap();
    assert_eq!(outputs.status, "failed");
    assert!(!outputs.found);
    assert_eq!(outputs.info.context_window, None);
    let error = outputs.error.unwrap();
    assert!(error.contains("HTTP 403"), "{error}");
    assert!(!error.contains("hidden-provider-detail"), "{error}");
}

#[test]
fn model_info_node_fails_fast_on_missing_credentials_and_preserves_cancellation() {
    let mut slow = Reply::models(json!([{"id": "example-model", "context_window": 32768}]));
    slow.header_delay = Duration::from_secs(2);
    let server = Server::start_models(vec![slow]);
    let registry = NodeRegistry::from_inventory().unwrap();
    let factory = registry.get(MODEL_INFO_KIND).unwrap();

    // Missing environment credential fails before any request, even with capture_errors.
    let prepared = factory
        .instantiate(json!({
            "model": "example-model",
            "base_url": server.base_url,
            "api_key_env": "MINIFLOW_LLM_MODEL_INFO_MISSING_KEY",
            "capture_errors": true
        }))
        .unwrap();
    let mut context = ExecutionContext::default();
    let error = prepared
        .execution
        .as_task_node()
        .unwrap()
        .execute(Inputs::new(), &mut context)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("MINIFLOW_LLM_MODEL_INFO_MISSING_KEY")
    );
    assert!(server.model_requests().is_empty());

    // Cancellation is not swallowed by capture_errors.
    let prepared = factory
        .instantiate(json!({
            "model": "example-model",
            "base_url": server.base_url,
            "api_key": {"$ref": "params.api_key"},
            "capture_errors": true
        }))
        .unwrap();
    let mut context = context_with_key("");
    let cancellation = StreamCancellation::default();
    cancellation.cancel(
        mf_runtime::StreamExecutionSnafu {
            message: "cancelled model info test",
        }
        .build(),
    );
    context.set_cancellation(cancellation);
    let error = prepared
        .execution
        .as_task_node()
        .unwrap()
        .execute(Inputs::new(), &mut context)
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error}");
    assert!(server.model_requests().is_empty());
}

#[test]
fn list_models_preserves_provider_order_normalizes_entries_and_deduplicates_ids() {
    let server = Server::start_models(vec![Reply::models(json!([
        {"id": ""},
        {"id": "   "},
        {"owned_by": "missing-id"},
        {
            "id": "first-model",
            "name": "First Model",
            "context_length": 131072,
            "max_output_tokens": 8192
        },
        {
            "id": "second-model",
            "owned_by": "provider"
        },
        {
            "id": "first-model",
            "context_length": 4096
        }
    ]))]);
    let mut query = ModelListQuery::new(&server.base_url);
    query.api_key = Some("list-key".into());
    let models = list_models(&query, &ExecutionContext::default()).unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "first-model");
    assert_eq!(models[0].name.as_deref(), Some("First Model"));
    assert_eq!(models[0].context_window, Some(131_072));
    assert_eq!(models[0].max_output_tokens, Some(8_192));
    assert_eq!(models[1].id, "second-model");
    assert_eq!(models[1].owned_by.as_deref(), Some("provider"));
    assert_eq!(models[1].context_window, None);
    let requests = server.model_requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .headers
            .to_ascii_lowercase()
            .contains("authorization: bearer list-key")
    );
}

#[test]
fn query_model_info_enforces_timeout() {
    let mut slow = Reply::models(json!([{"id": "example-model", "context_window": 32768}]));
    slow.header_delay = Duration::from_millis(500);
    let server = Server::start_models(vec![slow]);
    let mut query = ModelInfoQuery::new("example-model", &server.base_url);
    query.timeout_ms = 50;
    let error = query_model_info(&query, &ExecutionContext::default()).unwrap_err();
    assert!(
        matches!(error, LlmExecutionError::Future { .. }),
        "{error:?}"
    );
}
