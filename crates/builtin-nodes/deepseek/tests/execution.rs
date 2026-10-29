mod support;

use mf_runtime::{ExecutionContext, Inputs, NodeRegistry, StreamCancellation, StreamError};
use mfn_deepseek::{FETCH_KIND, SEARCH_KIND};
use serde_json::{Value, json};
use std::{error::Error, process::Command, thread, time::Duration};
use support::{Reply, Server};

fn probe(kind: &str, server: &Server, input: Value, patch: Value) -> Value {
    let mut config = json!({
        "base_url": server.base_url,
        "api_key_env": "MINIFLOW_DEEPSEEK_TEST_KEY"
    });
    config
        .as_object_mut()
        .unwrap()
        .extend(patch.as_object().unwrap().clone());
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "execution_probe", "--ignored", "--nocapture"])
        .env("MINIFLOW_DEEPSEEK_PROBE_CONFIG", config.to_string())
        .env("MINIFLOW_DEEPSEEK_PROBE_KIND", kind)
        .env("MINIFLOW_DEEPSEEK_PROBE_INPUT", input.to_string())
        .env("MINIFLOW_DEEPSEEK_TEST_KEY", "synthetic-key")
        .env_remove("MINIFLOW_DEEPSEEK_UNAVAILABLE_CREDENTIAL")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost");
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(name);
    }
    if let Some(credential) = input["credential"].as_str() {
        command.env("MINIFLOW_DEEPSEEK_TEST_KEY", credential);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    serde_json::from_str(
        text.lines()
            .find_map(|line| line.strip_prefix("DEEPSEEK_RESULT:"))
            .unwrap(),
    )
    .unwrap()
}

fn values(input: &Value) -> Inputs {
    input
        .as_object()
        .unwrap()
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "credential" | "cancel"))
        .map(|(name, value)| (name.clone(), value.clone().into()))
        .collect()
}

fn failure(error: mf_runtime::NodeExecutionError) -> Value {
    let mut source: &dyn Error = &error;
    let mut typed_credentials = false;
    let mut typed_runtime = false;
    while let Some(next) = source.source() {
        typed_credentials |= next.is::<std::env::VarError>();
        typed_runtime |= next.is::<StreamError>()
            || next.is::<mf_runtime::FutureExecutionError>()
            || matches!(
                next.downcast_ref::<mfn_deepseek::DeepSeekExecutionError>(),
                Some(mfn_deepseek::DeepSeekExecutionError::Future { .. })
            );
        source = next;
    }
    json!({
        "ok": false,
        "error": error.to_string(),
        "debug": format!("{error:?}"),
        "cause": source.to_string(),
        "typed_credentials": typed_credentials,
        "typed_runtime": typed_runtime
    })
}

#[test]
#[ignore = "invoked as an isolated credential subprocess"]
fn execution_probe() {
    let config: Value =
        serde_json::from_str(&std::env::var("MINIFLOW_DEEPSEEK_PROBE_CONFIG").unwrap()).unwrap();
    let input: Value =
        serde_json::from_str(&std::env::var("MINIFLOW_DEEPSEEK_PROBE_INPUT").unwrap()).unwrap();
    let kind = std::env::var("MINIFLOW_DEEPSEEK_PROBE_KIND").unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let prepared = registry.get(&kind).unwrap().instantiate(config).unwrap();
    let node = prepared.execution.as_task_node().unwrap();
    let cancellation = StreamCancellation::default();
    if input["cancel"] == true {
        let cancellation = cancellation.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            cancellation.cancel(StreamError::Execution {
                message: "requested cancellation".into(),
            });
        });
    }
    let mut context = ExecutionContext::default();
    context.set_cancellation(cancellation);
    let result = match node.execute(values(&input), &mut context) {
        Ok(result) => json!({"ok":true,"outputs":result.outputs}),
        Err(error) => failure(error),
    };
    println!("DEEPSEEK_RESULT:{result}");
}

fn search_result() -> Value {
    json!({
        "rerank_score":2.4,
        "title":"DeepSeek release",
        "url":"https://example.com/release",
        "snippet":"Short snippet",
        "summary":"Long summary",
        "content":"# Full content",
        "site_name":null,
        "site_icon":null,
        "date":null,
        "authority":3
    })
}

#[test]
fn search_sends_all_options_and_preserves_the_complete_result() {
    let expected = search_result();
    let server = Server::start(vec![Reply::search(json!([expected.clone()]))]);
    let result = probe(
        SEARCH_KIND,
        &server,
        json!({
            "query":"DeepSeek site:example.com",
            "count":5,
            "ori_question":"What was released?",
            "localization":"127.0.0.1",
            "need_content":true
        }),
        json!({}),
    );
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["outputs"]["results"], json!([expected]));
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body,
        json!({
            "query":"DeepSeek site:example.com",
            "count":5,
            "ori_question":"What was released?",
            "localization":"127.0.0.1",
            "need_content":true
        })
    );
    assert!(
        requests[0]
            .headers
            .starts_with("POST /prefix/v1/tools/search HTTP/1.1")
    );
    assert!(
        requests[0]
            .headers
            .to_ascii_lowercase()
            .contains("authorization: bearer synthetic-key")
    );
    assert!(
        requests[0]
            .headers
            .to_ascii_lowercase()
            .contains("content-type: application/json")
    );
}

#[test]
fn search_defaults_and_empty_results_are_successful() {
    let server = Server::start(vec![Reply::search(json!([]))]);
    let result = probe(
        SEARCH_KIND,
        &server,
        json!({"query":"\u{6ca1}\u{6709}\u{7ed3}\u{679c}"}),
        json!({}),
    );
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["outputs"]["results"], json!([]));
    assert_eq!(
        server.requests()[0].body,
        json!({"query":"\u{6ca1}\u{6709}\u{7ed3}\u{679c}","count":10,"need_content":false})
    );
}

#[test]
fn fetch_preserves_title_and_markdown_content() {
    let server = Server::start(vec![Reply::fetch(
        "Example",
        "# Heading\n\n\u{6b63}\u{6587}",
    )]);
    let result = probe(
        FETCH_KIND,
        &server,
        json!({"url":"https://example.com/article?q=one#section"}),
        json!({}),
    );
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(
        result["outputs"],
        json!({"title":"Example","content":"# Heading\n\n\u{6b63}\u{6587}"})
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body,
        json!({"url":"https://example.com/article?q=one#section"})
    );
    assert!(
        requests[0]
            .headers
            .starts_with("POST /prefix/v1/tools/fetch HTTP/1.1")
    );
}

#[test]
fn invalid_inputs_fail_before_credentials_or_network_io() {
    let server = Server::start(vec![]);
    for (kind, input) in [
        (SEARCH_KIND, json!({"query":""})),
        (SEARCH_KIND, json!({"query":" "})),
        (SEARCH_KIND, json!({"query":"é".repeat(129)})),
        (SEARCH_KIND, json!({"query":"valid","count":0})),
        (SEARCH_KIND, json!({"query":"valid","count":51})),
        (SEARCH_KIND, json!({"query":"valid","ori_question":""})),
        (
            SEARCH_KIND,
            json!({"query":"valid","ori_question":"x".repeat(129)}),
        ),
        (FETCH_KIND, json!({"url":"/relative"})),
        (FETCH_KIND, json!({"url":"file:///tmp/article"})),
        (
            FETCH_KIND,
            json!({"url":"https://user:secret@example.com/article"}),
        ),
        (
            FETCH_KIND,
            json!({"url":format!("https://example.com/{}", "a".repeat(8192))}),
        ),
    ] {
        let result = probe(
            kind,
            &server,
            input,
            json!({"api_key_env":"MINIFLOW_DEEPSEEK_UNAVAILABLE_CREDENTIAL"}),
        );
        assert_eq!(result["ok"], false, "{result}");
        assert_eq!(result["typed_credentials"], false, "{result}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn missing_and_empty_credentials_fail_without_network_io() {
    let server = Server::start(vec![]);
    let result = probe(
        SEARCH_KIND,
        &server,
        json!({"query":"valid"}),
        json!({"api_key_env":"MINIFLOW_DEEPSEEK_UNAVAILABLE_CREDENTIAL"}),
    );
    assert_eq!(result["ok"], false, "{result}");
    assert_eq!(result["typed_credentials"], true, "{result}");
    let empty = probe(
        SEARCH_KIND,
        &server,
        json!({"query":"valid","credential":""}),
        json!({}),
    );
    assert_eq!(empty["ok"], false, "{empty}");
    assert!(empty["error"].as_str().unwrap().contains("is empty"));
    assert!(server.requests().is_empty());
}

#[test]
fn provider_errors_are_not_retried_and_diagnostics_hide_the_body() {
    for status in [400, 401, 402, 403, 404, 408, 429, 500, 504] {
        let mut reply = Reply::json(json!({
            "error":{"message":"synthetic-key remote-secret","type":"provider"}
        }));
        reply.status = status;
        let server = Server::start(vec![reply]);
        let result = probe(
            FETCH_KIND,
            &server,
            json!({"url":"https://example.com"}),
            json!({}),
        );
        assert_eq!(result["ok"], false, "{result}");
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains(&format!("HTTP {status}")),
            "{result}"
        );
        for field in ["error", "debug"] {
            assert!(!result[field].as_str().unwrap().contains("synthetic-key"));
            assert!(!result[field].as_str().unwrap().contains("remote-secret"));
        }
        assert_eq!(server.requests().len(), 1);
    }
}

#[test]
fn malformed_successful_responses_publish_nothing() {
    let server = Server::start(vec![Reply::json(json!({"unexpected":true}))]);
    let result = probe(SEARCH_KIND, &server, json!({"query":"valid"}), json!({}));
    assert_eq!(result["ok"], false, "{result}");
    assert!(result.get("outputs").is_none());
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn redirects_do_not_contact_the_target() {
    let target = Server::start(vec![Reply::fetch("unexpected", "unexpected")]);
    let mut reply = Reply::json(json!({"message":"redirect"}));
    reply.status = 307;
    reply.headers.push((
        "Location".into(),
        format!("{}/v1/tools/fetch", target.base_url),
    ));
    let server = Server::start(vec![reply]);
    let result = probe(
        FETCH_KIND,
        &server,
        json!({"url":"https://example.com"}),
        json!({}),
    );
    assert_eq!(result["ok"], false, "{result}");
    assert_eq!(server.requests().len(), 1);
    assert!(target.requests().is_empty());
}

#[test]
fn waiting_is_bounded_and_cancelled_calls_publish_nothing() {
    for body in [false, true] {
        let mut reply = Reply::search(json!([]));
        if body {
            reply.body_delay = Duration::from_millis(500);
        } else {
            reply.header_delay = Duration::from_millis(500);
        }
        let server = Server::start(vec![reply]);
        let result = probe(
            SEARCH_KIND,
            &server,
            json!({"query":"valid"}),
            json!({"timeout_ms":150}),
        );
        assert_eq!(result["ok"], false, "{result}");
        assert!(result.get("outputs").is_none());
    }
    let mut reply = Reply::search(json!([]));
    reply.body_delay = Duration::from_millis(800);
    let server = Server::start(vec![reply]);
    let result = probe(
        SEARCH_KIND,
        &server,
        json!({"query":"valid","cancel":true}),
        json!({}),
    );
    assert_eq!(result["ok"], false, "{result}");
    assert_eq!(result["typed_runtime"], true, "{result}");
    assert!(result.get("outputs").is_none());
}

#[test]
fn factories_reflect_both_contracts_without_execution() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (kind, input_count) in [(SEARCH_KIND, 5), (FETCH_KIND, 1)] {
        let prepared = registry.get(kind).unwrap().instantiate(json!({})).unwrap();
        assert_eq!(prepared.metadata.ports.inputs.len(), input_count);
        assert!(prepared.execution.as_task_node().is_some());
        assert!(prepared.metadata.stdin.is_none());
    }
}
