mod support;

use mf_runtime::{ExecutionContext, Inputs, NodeRegistry, StreamCancellation, StreamError};
use mfn_llm::{KIND, LlmExecutionError, prepare_llm};
use serde_json::{Value, json};
use std::{error::Error, process::Command, thread, time::Duration};
use support::{Reply, Server, messages};

fn probe(server: &Server, inputs: Value, patch: Value) -> Value {
    let mut config = json!({"model": "example-model", "base_url": server.base_url, "api_key_env": "MINIFLOW_LLM_TEST_KEY"});
    config
        .as_object_mut()
        .unwrap()
        .extend(patch.as_object().unwrap().clone());
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "execution_probe", "--ignored", "--nocapture"])
        .env("MINIFLOW_LLM_PROBE_CONFIG", config.to_string())
        .env("MINIFLOW_LLM_PROBE_INPUT", inputs.to_string())
        .env("MINIFLOW_LLM_TEST_KEY", "synthetic-key")
        .env_remove("MINIFLOW_LLM_UNAVAILABLE_CREDENTIAL")
        .env("OPENAI_BASE_URL", "http://127.0.0.1:1/ignored")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy");
    if let Some(credential) = inputs["credential"].as_str() {
        command.env("MINIFLOW_LLM_TEST_KEY", credential);
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
            .find_map(|line| line.strip_prefix("LLM_RESULT:"))
            .unwrap(),
    )
    .unwrap()
}

fn values(input: &Value) -> Inputs {
    input
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, value)| (name.clone(), value.clone().into()))
        .collect()
}

fn failure(error: mf_runtime::NodeExecutionError) -> Value {
    let mut source: &dyn Error = &error;
    let mut typed_provider = false;
    let mut typed_runtime = false;
    let mut typed_credentials = false;
    while let Some(next) = source.source() {
        typed_provider |= next.is::<rig_core::error::ProviderError>()
            || next.is::<Box<rig_core::error::ProviderError>>();
        typed_runtime |= next.is::<mf_runtime::FutureExecutionError>()
            || matches!(
                next.downcast_ref::<LlmExecutionError>(),
                Some(LlmExecutionError::Future { .. })
            );
        typed_credentials |= next.is::<std::env::VarError>();
        source = next;
    }
    json!({"ok": false, "error": error.to_string(), "debug": format!("{error:?}"), "cause": source.to_string(), "typed_provider": typed_provider, "typed_runtime": typed_runtime, "typed_credentials": typed_credentials})
}

#[test]
#[ignore = "invoked as an isolated credential subprocess"]
fn execution_probe() {
    let config: Value =
        serde_json::from_str(&std::env::var("MINIFLOW_LLM_PROBE_CONFIG").unwrap()).unwrap();
    let parameters: Value =
        serde_json::from_str(&std::env::var("MINIFLOW_LLM_PROBE_INPUT").unwrap()).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let prepared = registry.get(KIND).unwrap().instantiate(config).unwrap();
    let node = prepared.execution.as_task_node().unwrap();
    let mut results = Vec::new();
    if parameters["parallel"] == true {
        let context = ExecutionContext::default();
        results = thread::scope(|scope| {
            let jobs = parameters["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|input| {
                    let mut context = context.fork_domain_with_observation(None);
                    scope.spawn(move || match node.execute(values(input), &mut context) {
                        Ok(result) => json!({"ok": true, "outputs": result.outputs}),
                        Err(error) => failure(error),
                    })
                })
                .collect::<Vec<_>>();
            jobs.into_iter().map(|job| job.join().unwrap()).collect()
        });
    } else {
        let cancellation = StreamCancellation::default();
        if parameters["cancel"] == true {
            let cancellation = cancellation.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(300));
                cancellation.cancel(StreamError::Execution {
                    message: "requested cancellation".into(),
                });
            });
        }
        let mut history: Option<Value> = None;
        let mut context = ExecutionContext::default();
        context.set_cancellation(cancellation.clone());
        for input in parameters["inputs"].as_array().unwrap() {
            if parameters["fresh_contexts"] == true {
                context = ExecutionContext::default();
                context.set_cancellation(cancellation.clone());
            }
            let mut input = input.clone();
            if parameters["carry_history"] == true
                && let Some(history) = &history
            {
                input["history"] = history.clone();
            }
            match node.execute(values(&input), &mut context) {
                Ok(result) => {
                    let output = serde_json::to_value(result.outputs).unwrap();
                    history = Some(output["history"].clone());
                    results.push(json!({"ok": true, "outputs": output}));
                }
                Err(error) => results.push(failure(error)),
            }
        }
    }
    println!(
        "LLM_RESULT:{}",
        json!({"results": results, "original_inputs": parameters["inputs"]})
    );
}

#[test]
fn two_turns_reuse_the_node_and_preserve_explicit_history() {
    let server = Server::start(vec![
        Reply::text("first reply"),
        Reply::text("second reply"),
    ]);
    let initial = json!([{"role": "user", "content": "earlier"}, {"role": "assistant", "content": "prior reply", "metadata": {"note":"local-annotation"}}]);
    let result = probe(
        &server,
        json!({"inputs": [{"prompt": "first", "system": "be brief", "history": initial}, {"prompt": "second", "system": "be brief"}], "carry_history": true, "fresh_contexts": true}),
        json!({"temperature": 0.2, "max_tokens": 7}),
    );
    let turns = result["results"].as_array().unwrap();
    assert!(turns.iter().all(|turn| turn["ok"] == true), "{result}");
    assert_eq!(turns[0]["outputs"]["history"].as_array().unwrap().len(), 4);
    assert_eq!(turns[1]["outputs"]["history"].as_array().unwrap().len(), 6);
    assert_eq!(result["original_inputs"][0]["history"], initial);
    assert_eq!(
        turns[1]["outputs"]["history"][1]["metadata"],
        initial[1]["metadata"]
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| !request.body.to_string().contains("local-annotation"))
    );
    for request in &requests {
        assert!(
            request
                .headers
                .starts_with("POST /prefix/v1/chat/completions HTTP/1.1")
        );
        assert!(
            request
                .headers
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-key")
        );
        assert_ne!(request.body["stream"], true);
        assert_eq!(request.body["model"], "example-model");
        assert_eq!(request.body["temperature"], 0.2);
        assert_eq!(request.body["max_tokens"], 7);
    }
    let expected = json!([
        {"role": "system", "content": "be brief"},
        {"role": "user", "content": "earlier"}, {"role": "assistant", "content": "prior reply"},
        {"role": "user", "content": "first"}, {"role": "assistant", "content": "first reply"},
        {"role": "user", "content": "second"}
    ]);
    assert_eq!(messages(&requests[1]), expected.as_array().unwrap().clone());
}

#[test]
fn separate_invocations_have_no_implicit_conversation_memory() {
    let server = Server::start(vec![
        Reply::text("one"),
        Reply::text("two"),
        Reply::text("three"),
    ]);
    let result = probe(
        &server,
        json!({"inputs": [{"prompt": "a", "history": [{"role": "assistant", "content": "previous"}]}, {"prompt": "b", "system": ""}, {"prompt": "c", "history": []}]}),
        json!({}),
    );
    assert!(
        result["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|turn| turn["ok"] == true),
        "{result}"
    );
    let requests = server.requests();
    assert_eq!(
        messages(&requests[1]),
        vec![json!({"role": "user", "content": "b"})]
    );
    assert_eq!(
        messages(&requests[2]),
        vec![json!({"role": "user", "content": "c"})]
    );
}

#[test]
fn parallel_calls_use_independent_transcripts() {
    let server = Server::start(vec![Reply::text("reply"); 3]);
    let result = probe(
        &server,
        json!({"parallel": true, "inputs": [{"prompt": "a"}, {"prompt": "b"}, {"prompt": "c"}]}),
        json!({}),
    );
    for (turn, prompt) in result["results"]
        .as_array()
        .unwrap()
        .iter()
        .zip(["a", "b", "c"])
    {
        assert_eq!(turn["ok"], true, "{result}");
        assert_eq!(turn["outputs"]["history"][0]["content"], prompt);
        assert_eq!(turn["outputs"]["history"].as_array().unwrap().len(), 2);
    }
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn invalid_history_and_prompts_fail_before_network_io() {
    let server = Server::start(vec![]);
    for (input, path) in [
        (json!({"prompt": ""}), "/prompt"),
        (
            json!({"prompt": "next", "history": [{"role": "system", "content": "bad"}]}),
            "/history/0/role",
        ),
        (
            json!({"prompt": "next", "history": [{"role": "tool", "content": "bad"}]}),
            "/history/0/role",
        ),
        (
            json!({"prompt": "next", "history": [{"role": "user", "content": ""}]}),
            "/history/0/content",
        ),
        (
            json!({"prompt": "next", "history": [{"role": "user", "content": 42}]}),
            "/history/0/content",
        ),
        (json!({"prompt": "next", "history": null}), "/history"),
    ] {
        let result = probe(&server, json!({"inputs": [input]}), json!({}));
        let failure = &result["results"][0];
        assert_eq!(failure["ok"], false);
        assert!(
            failure["error"].as_str().unwrap().contains(path),
            "{failure}"
        );
        assert!(server.requests().is_empty());
    }
}

#[test]
fn missing_and_empty_credentials_fail_without_acquiring_network_resources() {
    let server = Server::start(vec![]);
    let result = probe(
        &server,
        json!({"inputs": [{"prompt": "hello"}]}),
        json!({"api_key_env": "MINIFLOW_LLM_UNAVAILABLE_CREDENTIAL"}),
    );
    assert_eq!(result["results"][0]["ok"], false);
    assert_eq!(result["results"][0]["typed_credentials"], true);
    assert!(server.requests().is_empty());
    let empty = probe(
        &server,
        json!({"credential": "", "inputs": [{"prompt": "hello"}]}),
        json!({}),
    );
    assert_eq!(empty["results"][0]["ok"], false);
    assert!(
        empty["results"][0]["error"]
            .as_str()
            .unwrap()
            .contains("is empty")
    );
    assert!(server.requests().is_empty());
    let prepared =
        prepare_llm(json!({"model": "example-model", "base_url": server.base_url})).unwrap();
    assert_eq!(prepared.metadata.ports.inputs.len(), 3);
}

#[test]
fn provider_errors_are_not_retried_and_diagnostics_hide_the_body() {
    for status in [401, 429, 500] {
        let mut reply = Reply::json(
            json!({"error": {"message": "synthetic-key remote-secret", "type": "provider", "code": "failure"}}),
        );
        reply.status = status;
        let server = Server::start(vec![reply]);
        let result = probe(&server, json!({"inputs": [{"prompt": "hello"}]}), json!({}));
        let failure = &result["results"][0];
        assert_eq!(failure["ok"], false);
        assert_eq!(failure["typed_provider"], true);
        assert!(
            failure["error"]
                .as_str()
                .unwrap()
                .contains(&format!("HTTP {status}")),
            "{failure}"
        );
        for field in ["error", "debug"] {
            assert!(!failure[field].as_str().unwrap().contains("synthetic-key"));
            assert!(!failure[field].as_str().unwrap().contains("remote-secret"));
        }
        assert_eq!(server.requests().len(), 1);
    }
}

#[test]
fn redirects_do_not_contact_the_target() {
    let target = Server::start(vec![Reply::text("unexpected")]);
    let mut reply = Reply::json(json!({"message": "redirect"}));
    reply.status = 307;
    reply.headers.push((
        "Location".into(),
        format!("{}/chat/completions", target.base_url),
    ));
    let server = Server::start(vec![reply]);
    let result = probe(&server, json!({"inputs": [{"prompt": "hello"}]}), json!({}));
    assert_eq!(result["results"][0]["ok"], false);
    assert_eq!(server.requests().len(), 1);
    assert!(target.requests().is_empty());
}

#[test]
fn waiting_for_headers_or_body_is_bounded_and_cancelled_calls_publish_nothing() {
    for body in [false, true] {
        let mut reply = Reply::text("too late");
        if body {
            reply.body_delay = Duration::from_millis(500);
        } else {
            reply.header_delay = Duration::from_millis(500);
        }
        let server = Server::start(vec![reply]);
        let result = probe(
            &server,
            json!({"inputs": [{"prompt": "hello"}]}),
            json!({"timeout_ms": 150}),
        );
        assert_eq!(result["results"][0]["ok"], false);
        assert!(result["results"][0].get("outputs").is_none());
    }
    let mut reply = Reply::text("too late");
    reply.body_delay = Duration::from_millis(800);
    let server = Server::start(vec![reply]);
    let result = probe(
        &server,
        json!({"cancel": true, "inputs": [{"prompt": "hello"}]}),
        json!({}),
    );
    assert_eq!(result["results"][0]["ok"], false);
    assert_eq!(result["results"][0]["typed_runtime"], true);
}

#[test]
fn empty_tool_and_malformed_responses_fail_without_history() {
    let mut tool = serde_json::from_str::<Value>(&Reply::text("text").body).unwrap();
    tool["choices"][0]["message"]["tool_calls"] = json!([{"id": "call", "type": "function", "function": {"name": "noop", "arguments": "{}"}}]);
    for (reply, config) in [
        (Reply::text(""), json!({})),
        (Reply::json(tool), json!({})),
        (
            Reply {
                body: "invalid json".into(),
                ..Reply::text("unused")
            },
            json!({}),
        ),
    ] {
        let server = Server::start(vec![reply]);
        let result = probe(
            &server,
            json!({"inputs": [{"prompt": "hello", "history": [{"role": "user", "content": "prior"}]}]}),
            config,
        );
        assert_eq!(result["results"][0]["ok"], false);
        assert!(result["results"][0].get("outputs").is_none());
        assert_eq!(
            result["original_inputs"][0]["history"][0]["content"],
            "prior"
        );
    }
}

#[test]
fn sparse_usage_unknown_finish_and_token_truncation_are_preserved() {
    for (usage, reason) in [
        (json!({"completion_tokens": 0}), "service_stop"),
        (Value::Null, "length"),
        (json!({"prompt_tokens":2,"completion_tokens":1}), "stop"),
    ] {
        let mut reply = serde_json::from_str::<Value>(&Reply::text("partial").body).unwrap();
        if usage.is_null() {
            reply.as_object_mut().unwrap().remove("usage");
        } else {
            reply["usage"] = usage;
        }
        reply["choices"][0]["finish_reason"] = reason.into();
        let server = Server::start(vec![Reply::json(reply)]);
        let result = probe(&server, json!({"inputs": [{"prompt": "hello"}]}), json!({}));
        let outputs = &result["results"][0]["outputs"];
        assert_eq!(outputs["text"], "partial", "{result}");
        assert_eq!(outputs["finish_reason"], reason);
        if reason == "service_stop" {
            assert_eq!(outputs["usage"], json!({"output_tokens": 0}));
        } else if reason == "stop" {
            assert_eq!(
                outputs["usage"],
                json!({"input_tokens":2,"output_tokens":1})
            );
            assert_eq!(outputs["usages"][0]["usage"], outputs["usage"]);
        } else {
            assert!(outputs.get("usage").is_none());
        }
    }
}

#[test]
fn missing_metadata_is_omitted_and_invalid_counts_do_not_publish() {
    let mut payload = serde_json::from_str::<Value>(&Reply::text("reply").body).unwrap();
    payload.as_object_mut().unwrap().remove("usage");
    payload["choices"][0]
        .as_object_mut()
        .unwrap()
        .remove("finish_reason");
    let server = Server::start(vec![Reply::json(payload)]);
    let result = probe(
        &server,
        json!({"inputs": [{"prompt": "next", "history": [{"role": "user", "content": "one"}, {"role": "user", "content": "two"}]}]}),
        json!({}),
    );
    assert_eq!(result["results"][0]["ok"], true, "{result}");
    let outputs = &result["results"][0]["outputs"];
    assert!(outputs.get("usage").is_none());
    assert!(outputs.get("finish_reason").is_none());
    assert_eq!(outputs["history"][0]["content"], "one");
    assert_eq!(outputs["history"][1]["content"], "two");

    for usage in [
        json!({"completion_tokens": i64::MAX as u64 + 1}),
        json!({"completion_tokens": "synthetic-key"}),
    ] {
        let mut payload = serde_json::from_str::<Value>(&Reply::text("reply").body).unwrap();
        payload["usage"] = usage;
        let server = Server::start(vec![Reply::json(payload)]);
        let result = probe(&server, json!({"inputs": [{"prompt": "next"}]}), json!({}));
        let failure = &result["results"][0];
        assert_eq!(failure["ok"], false);
        assert!(failure.get("outputs").is_none());
        assert!(!failure["error"].as_str().unwrap().contains("synthetic-key"));
        assert!(!failure["debug"].as_str().unwrap().contains("synthetic-key"));
    }
}

#[test]
fn error_sources_remain_typed_without_raw_diagnostic_display() {
    let error = LlmExecutionError::Provider {
        kind: "provider_response",
        status: Some(401),
        source: Box::new(rig_core::error::ProviderError::from_http_response(
            rig_core::http_client::StatusCode::UNAUTHORIZED,
            "remote-secret",
        )),
    };
    assert!(!format!("{error:?}").contains("remote-secret"));
    let source = error
        .source()
        .unwrap()
        .downcast_ref::<Box<rig_core::error::ProviderError>>()
        .unwrap();
    assert_eq!(source.provider_response_status().unwrap().as_u16(), 401);
}

#[test]
fn captured_provider_failures_and_timeouts_return_explicit_call_facts() {
    let mut failed = Reply::json(json!({"error":{"message":"remote-secret"}}));
    failed.status = 503;
    let mut timed_out = Reply::text("too late");
    timed_out.header_delay = Duration::from_millis(300);
    for (response, timeout, expected_error) in [
        (failed, 60_000, "HTTP 503"),
        (timed_out, 100, "deadline exceeded"),
    ] {
        let server = Server::start(vec![response]);
        let result = probe(
            &server,
            json!({"inputs":[{"prompt":"hello"}]}),
            json!({"capture_errors":true,"timeout_ms":timeout}),
        );
        let outputs = &result["results"][0]["outputs"];
        assert_eq!(result["results"][0]["ok"], true, "{result}");
        assert_eq!(outputs["status"], "failed");
        assert_eq!(outputs["text"], "");
        assert_eq!(outputs["usages"].as_array().unwrap().len(), 1);
        assert_eq!(outputs["usages"][0]["model"], "example-model");
        assert!(!outputs["usages"][0]["call_id"].as_str().unwrap().is_empty());
        assert!(outputs["usages"][0].get("usage").is_none());
        assert_eq!(outputs["usages"][0]["error"], outputs["error"]);
        assert!(
            outputs["error"].as_str().unwrap().contains(expected_error),
            "{outputs}"
        );
        assert!(!outputs.to_string().contains("remote-secret"));
        if expected_error == "HTTP 503" {
            assert_eq!(server.requests().len(), 1);
        } else {
            // A deadline can expire before the request reaches the server.
            assert!(server.requests().len() <= 1);
        }
    }
}

#[test]
fn captured_response_validation_failure_keeps_reported_tokens() {
    let server = Server::start(vec![Reply::text("")]);
    let result = probe(
        &server,
        json!({"inputs":[{"prompt":"hello"}]}),
        json!({"capture_errors":true}),
    );
    let outputs = &result["results"][0]["outputs"];
    assert_eq!(outputs["status"], "failed");
    assert_eq!(
        outputs["usages"][0]["usage"],
        json!({"input_tokens":2,"output_tokens":1,"total_tokens":3})
    );
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn attribution_is_reported_for_success_and_captured_failure_without_entering_provider_payloads() {
    let mut failed = Reply::json(json!({"error":{"message":"unavailable"}}));
    failed.status = 503;
    let server = Server::start(vec![Reply::text("answer"), failed]);
    let attribution = json!({"purpose":"title", "profile":"summary-profile"});
    let result = probe(
        &server,
        json!({"inputs":[{"prompt":"first"},{"prompt":"second"}]}),
        json!({"capture_errors":true,"attribution":attribution}),
    );
    for (index, status) in ["succeeded", "failed"].into_iter().enumerate() {
        assert_eq!(result["results"][index]["outputs"]["status"], status);
        assert_eq!(
            result["results"][index]["outputs"]["usages"][0]["attribution"],
            attribution
        );
    }
    for request in server.requests() {
        assert!(request.body.get("attribution").is_none());
        assert!(!request.body.to_string().contains("summary-profile"));
    }
}

#[test]
fn large_utf8_reply_is_preserved_without_a_client_output_byte_limit() {
    let text = "é".repeat(600_000);
    let server = Server::start(vec![Reply::text(&text)]);
    let result = probe(&server, json!({"inputs":[{"prompt":"hello"}]}), json!({}));
    let result = &result["results"][0];
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["outputs"]["text"].as_str().unwrap(), text);
    assert_eq!(
        result["outputs"]["history"][1]["content"].as_str().unwrap(),
        text
    );
    assert_eq!(server.requests().len(), 1);
}
