use mf_chat::{ChatHost, HostConfig, MessageUpdate, WorkflowProfile};
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{NodeValues, ValueRef, WorkflowArguments};
use mfn_llm::test_support::{Reply, Server, messages as request_messages};
use serde_json::{Value, json};

fn compact_wrapper_definition() -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version": "2026-11-13",
        "dependencies": {},
        "flows": {
            "compact": { "builtin": "mf-chat.chat-compact" }
        },
        "nodes": [
            { "id": "compact", "kind": "workflow.call", "config": { "flow": "compact" } }
        ],
        "outputs": [
            { "name": "status", "node": "compact", "port": "status" },
            { "name": "compacted", "node": "compact", "port": "compacted" },
            { "name": "messages", "node": "compact", "port": "messages" },
            { "name": "summary", "node": "compact", "port": "summary" },
            { "name": "usages", "node": "compact", "port": "usages" },
            { "name": "tokens_before", "node": "compact", "port": "tokens_before" },
            { "name": "read_files", "node": "compact", "port": "read_files" },
            { "name": "modified_files", "node": "compact", "port": "modified_files" },
            { "name": "error", "node": "compact", "port": "error" }
        ]
    }))
    .unwrap()
}

fn try_call_compact(
    arguments: WorkflowArguments,
) -> Result<NodeValues, mf_runtime::WorkflowRunError> {
    let definition = compact_wrapper_definition();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_with_inputs(arguments)
}

fn call_compact(arguments: Value) -> NodeValues {
    try_call_compact(WorkflowArguments::try_from(arguments).unwrap()).unwrap()
}

#[test]
fn skips_when_disabled_below_threshold_or_nothing_to_summarize() {
    let conversation = json!([
        { "role": "system", "content": "Be concise." },
        { "role": "user", "content": "Hello" },
        { "role": "assistant", "content": "Hi there" }
    ]);

    for (label, extra) in [
        ("disabled", json!({ "compact.params.enabled": false })),
        (
            "below_threshold",
            json!({
                "compact.params.context_window": 10000,
                "compact.params.reserve_tokens": 1000
            }),
        ),
        (
            "fits_keep_recent",
            json!({
                "compact.params.force": true,
                "compact.params.keep_recent_tokens": 20000
            }),
        ),
    ] {
        let mut args = json!({
            "compact.params.messages": conversation,
            "compact.params.api_key": "synthetic-key"
        });
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        let outputs = call_compact(args);
        assert_eq!(outputs["status"].as_str(), Some("skipped"), "{label}");
        assert_eq!(outputs["compacted"].as_bool(), Some(false), "{label}");
        assert_eq!(
            serde_json::to_value(&outputs["messages"]).unwrap(),
            conversation,
            "{label}"
        );
        assert_eq!(outputs["summary"].as_str(), Some(""), "{label}");
        assert_eq!(
            serde_json::to_value(&outputs["usages"]).unwrap(),
            json!([]),
            "{label}"
        );
        assert_eq!(outputs["error"].as_str(), Some(""), "{label}");
    }
}

#[test]
fn threshold_compaction_keeps_recent_turn_tracks_files_and_truncates_tool_output() {
    let server = Server::start(vec![Reply::text(
        "## Goal\nInspect and edit files\n\n## Next Steps\n1. Continue",
    )]);
    let long_tool = format!("{}{}", "a".repeat(2000), "b".repeat(50));
    let messages = json!([
        { "role": "system", "content": "System instructions" },
        { "role": "user", "content": "Inspect alpha and edit beta" },
        {
            "role": "assistant",
            "content": "Checking both files.",
            "tool_calls": [
                { "id": "call-1", "name": "read", "arguments": { "path": "src/alpha.rs" } },
                { "id": "call-2", "name": "read", "arguments": { "path": "src/beta.rs" } },
                { "id": "call-3", "name": "edit", "arguments": { "path": "src/beta.rs", "old": "1", "new": "2" } }
            ]
        },
        { "role": "tool", "tool_call_id": "call-1", "content": long_tool },
        { "role": "tool", "tool_call_id": "call-2", "content": "beta contents" },
        { "role": "tool", "tool_call_id": "call-3", "content": "edited" },
        { "role": "assistant", "content": "Finished inspecting alpha and editing beta." }
    ]);
    let append_messages = json!([
        { "role": "user", "content": "Recent question kept verbatim" },
        { "role": "assistant", "content": "Recent answer kept verbatim" }
    ]);
    let outputs = call_compact(json!({
        "compact.params.messages": messages,
        "compact.params.append_messages": append_messages,
        "compact.params.usages": [
            {
                "call_id": "turn-1",
                "model": "fixture-model",
                "attribution": { "purpose": "conversation" },
                "usage": { "input_tokens": 700, "output_tokens": 200, "total_tokens": 900 }
            }
        ],
        "compact.params.context_window": 1000,
        "compact.params.reserve_tokens": 200,
        "compact.params.keep_recent_tokens": 12,
        "compact.params.custom_instructions": "Preserve file paths.",
        "compact.params.model": "fixture-model",
        "compact.params.base_url": server.base_url,
        "compact.params.api_key": "synthetic-key"
    }));

    assert_eq!(outputs["status"].as_str(), Some("succeeded"));
    assert_eq!(outputs["compacted"].as_bool(), Some(true));
    assert_eq!(outputs["tokens_before"].as_i64(), Some(900));
    assert_eq!(
        serde_json::to_value(&outputs["read_files"]).unwrap(),
        json!(["src/alpha.rs"])
    );
    assert_eq!(
        serde_json::to_value(&outputs["modified_files"]).unwrap(),
        json!(["src/beta.rs"])
    );
    let summary = outputs["summary"].as_str().unwrap();
    assert!(summary.contains("<read-files>\nsrc/alpha.rs\n</read-files>"));
    assert!(summary.contains("<modified-files>\nsrc/beta.rs\n</modified-files>"));

    let rebuilt = serde_json::to_value(&outputs["messages"]).unwrap();
    let rebuilt = rebuilt.as_array().unwrap();
    assert_eq!(rebuilt.len(), 4);
    assert_eq!(rebuilt[0]["role"], "system");
    assert_eq!(rebuilt[0]["content"], "System instructions");
    assert_eq!(rebuilt[1]["role"], "user");
    assert!(
        rebuilt[1]["content"]
            .as_str()
            .unwrap()
            .contains("<summary>\n## Goal\nInspect and edit files")
    );
    assert_eq!(rebuilt[2]["content"], "Recent question kept verbatim");
    assert_eq!(rebuilt[3]["content"], "Recent answer kept verbatim");

    let usages = serde_json::to_value(&outputs["usages"]).unwrap();
    assert_eq!(usages.as_array().unwrap().len(), 1);
    assert_eq!(usages[0]["attribution"]["purpose"], "compaction");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let sent = request_messages(&requests[0]);
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["role"], "system");
    let prompt = sent[1]["content"].as_str().unwrap();
    assert!(prompt.contains("[User]: Inspect alpha and edit beta"));
    assert!(prompt.contains("[Assistant tool calls]: read(path=\"src/alpha.rs\")"));
    assert!(prompt.contains("[... 50 more characters truncated]"));
    assert!(prompt.contains("Additional focus: Preserve file paths."));
    assert!(!prompt.contains("Recent question kept verbatim"));
}

#[test]
fn split_turn_summarizes_history_then_turn_prefix_and_merges_usages() {
    let server = Server::start(vec![
        Reply::text("## Goal\nHistory summary"),
        Reply::text("## Original Request\nSplit turn checkpoint"),
    ]);
    let large = "x".repeat(400);
    let messages = json!([
        { "role": "user", "content": "Earlier turn" },
        { "role": "assistant", "content": "Earlier reply" },
        { "role": "user", "content": format!("Big current turn {large}") },
        {
            "role": "assistant",
            "content": "",
            "tool_calls": [{ "id": "t1", "name": "read", "arguments": { "path": "src/split.rs" } }]
        },
        { "role": "tool", "tool_call_id": "t1", "content": large },
        { "role": "assistant", "content": "Kept tail response" }
    ]);
    let outputs = call_compact(json!({
        "compact.params.messages": messages,
        "compact.params.enabled": false,
        "compact.params.force": true,
        "compact.params.keep_recent_tokens": 50,
        "compact.params.model": "fixture-model",
        "compact.params.base_url": server.base_url,
        "compact.params.api_key": "synthetic-key"
    }));

    assert_eq!(outputs["status"].as_str(), Some("succeeded"));
    assert_eq!(outputs["compacted"].as_bool(), Some(true));
    let summary = outputs["summary"].as_str().unwrap();
    assert!(summary.contains("## Goal\nHistory summary"));
    assert!(summary.contains("**Turn Context (split turn):**"));
    assert!(summary.contains("## Original Request\nSplit turn checkpoint"));
    assert!(summary.contains("<read-files>\nsrc/split.rs\n</read-files>"));

    let usages = serde_json::to_value(&outputs["usages"]).unwrap();
    assert_eq!(usages.as_array().unwrap().len(), 2);
    assert_eq!(usages[0]["attribution"]["purpose"], "compaction");
    assert_eq!(usages[1]["attribution"]["purpose"], "compaction");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        request_messages(&requests[0])[1]["content"]
            .as_str()
            .unwrap()
            .contains("[User]: Earlier turn")
    );
    assert!(
        request_messages(&requests[1])[1]["content"]
            .as_str()
            .unwrap()
            .contains("# Conversation\n[User]: Big current turn")
    );

    let rebuilt = serde_json::to_value(&outputs["messages"]).unwrap();
    let rebuilt = rebuilt.as_array().unwrap();
    assert_eq!(rebuilt.len(), 2);
    assert_eq!(rebuilt[1]["role"], "assistant");
    assert_eq!(rebuilt[1]["content"], "Kept tail response");
}

#[test]
fn split_turn_without_prior_history_only_calls_turn_prefix_model() {
    let server = Server::start(vec![Reply::text("## Original Request\nOnly split turn")]);
    let large = "y".repeat(400);
    let messages = json!([
        { "role": "user", "content": format!("First turn {large}") },
        { "role": "assistant", "content": "Kept assistant reply" }
    ]);
    let outputs = call_compact(json!({
        "compact.params.messages": messages,
        "compact.params.force": true,
        "compact.params.keep_recent_tokens": 4,
        "compact.params.model": "fixture-model",
        "compact.params.base_url": server.base_url,
        "compact.params.api_key": "synthetic-key"
    }));
    assert_eq!(outputs["status"].as_str(), Some("succeeded"));
    assert_eq!(server.requests().len(), 1);
    let summary = outputs["summary"].as_str().unwrap();
    assert!(summary.starts_with("No prior history.\n\n---\n\n**Turn Context (split turn):**"));
    assert!(summary.contains("## Original Request\nOnly split turn"));
}

#[test]
fn repeated_compaction_uses_update_prompt_and_cumulates_file_operations() {
    let server = Server::start(vec![
        Reply::text("## Goal\nFirst summary"),
        Reply::text("## Goal\nUpdated summary"),
    ]);
    let first = call_compact(json!({
        "compact.params.messages": [
            { "role": "system", "content": "Keep rules" },
            { "role": "user", "content": "Read file" },
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [{ "id": "r1", "name": "read", "arguments": { "path": "lib.rs" } }]
            },
            { "role": "tool", "tool_call_id": "r1", "content": "fn main() {}" },
            { "role": "assistant", "content": "Read lib.rs" }
        ],
        "compact.params.force": true,
        "compact.params.keep_recent_tokens": 0,
        "compact.params.model": "fixture-model",
        "compact.params.base_url": server.base_url,
        "compact.params.api_key": "synthetic-key"
    }));
    assert_eq!(first["status"].as_str(), Some("succeeded"));
    assert_eq!(
        serde_json::to_value(&first["read_files"]).unwrap(),
        json!(["lib.rs"])
    );
    assert_eq!(
        serde_json::to_value(&first["modified_files"]).unwrap(),
        json!([])
    );

    // Repeating immediately with no new messages skips compaction.
    let noop = call_compact(json!({
        "compact.params.messages": serde_json::to_value(&first["messages"]).unwrap(),
        "compact.params.force": true,
        "compact.params.keep_recent_tokens": 0,
        "compact.params.model": "fixture-model",
        "compact.params.base_url": server.base_url,
        "compact.params.api_key": "synthetic-key"
    }));
    assert_eq!(noop["status"].as_str(), Some("skipped"));
    assert_eq!(server.requests().len(), 1);

    // Adding a turn that writes lib.rs triggers iterative update and promotes lib.rs to modified.
    let second = call_compact(json!({
        "compact.params.messages": serde_json::to_value(&first["messages"]).unwrap(),
        "compact.params.append_messages": [
            { "role": "user", "content": "Write lib.rs" },
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [{ "id": "w1", "name": "write", "arguments": { "path": "lib.rs", "content": "pub fn x() {}" } }]
            },
            { "role": "tool", "tool_call_id": "w1", "content": "written" },
            { "role": "assistant", "content": "Wrote lib.rs" }
        ],
        "compact.params.force": true,
        "compact.params.keep_recent_tokens": 0,
        "compact.params.model": "fixture-model",
        "compact.params.base_url": server.base_url,
        "compact.params.api_key": "synthetic-key"
    }));
    assert_eq!(second["status"].as_str(), Some("succeeded"));
    assert_eq!(
        serde_json::to_value(&second["read_files"]).unwrap(),
        json!([])
    );
    assert_eq!(
        serde_json::to_value(&second["modified_files"]).unwrap(),
        json!(["lib.rs"])
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let update_messages = request_messages(&requests[1]);
    let update_prompt = update_messages[1]["content"].as_str().unwrap();
    assert!(
        update_prompt.contains("<previous-summary>\n## Goal\nFirst summary\n</previous-summary>")
    );
    assert!(update_prompt.contains("Update the existing structured summary"));
    let rebuilt = serde_json::to_value(&second["messages"]).unwrap();
    let rebuilt = rebuilt.as_array().unwrap();
    assert_eq!(rebuilt.len(), 2);
    assert_eq!(rebuilt[0]["content"], "Keep rules");
    assert!(
        rebuilt[1]["content"]
            .as_str()
            .unwrap()
            .contains("<modified-files>\nlib.rs\n</modified-files>")
    );
}

#[test]
fn failures_preserve_usages_keep_original_messages_and_short_circuit_split_turns() {
    let mut provider_error = Reply::json(json!({ "error": { "message": "summary unavailable" } }));
    provider_error.status = 503;

    let mut truncated = Reply::text("Partial summary");
    truncated.body = truncated.body.replace("\"stop\"", "\"length\"");

    let whitespace = Reply::text("   \n\t  ");

    for (label, reply, expected_error) in [
        ("provider_error", provider_error, "HTTP 503"),
        ("truncated", truncated, "token cap"),
        ("whitespace", whitespace, "complete nonempty summary"),
    ] {
        let server = Server::start(vec![reply]);
        let large = "z".repeat(400);
        let tail = "t".repeat(200);
        let conversation = json!([
            { "role": "user", "content": "Earlier turn" },
            { "role": "assistant", "content": "Earlier reply" },
            { "role": "user", "content": format!("Split turn {large}") },
            { "role": "assistant", "content": tail }
        ]);
        let outputs = call_compact(json!({
            "compact.params.messages": conversation,
            "compact.params.force": true,
            "compact.params.keep_recent_tokens": 40,
            "compact.params.model": "fixture-model",
            "compact.params.base_url": server.base_url,
            "compact.params.api_key": "synthetic-key"
        }));
        assert_eq!(outputs["status"].as_str(), Some("failed"), "{label}");
        assert_eq!(outputs["compacted"].as_bool(), Some(false), "{label}");
        assert_eq!(
            serde_json::to_value(&outputs["messages"]).unwrap(),
            conversation,
            "{label}"
        );
        assert_eq!(outputs["summary"].as_str(), Some(""), "{label}");
        assert!(
            outputs["error"].as_str().unwrap().contains(expected_error),
            "{label}: {}",
            outputs["error"]
        );
        let usages = serde_json::to_value(&outputs["usages"]).unwrap();
        assert_eq!(usages.as_array().unwrap().len(), 1, "{label}");
        assert_eq!(usages[0]["attribution"]["purpose"], "compaction", "{label}");
        // Must NOT call turn_prefix_model after history_model fails.
        assert_eq!(server.requests().len(), 1, "{label}");
    }
}

#[test]
fn split_turn_prefix_failure_retains_both_usages_and_original_messages() {
    let mut truncated = Reply::text("Incomplete turn prefix");
    truncated.body = truncated.body.replace("\"stop\"", "\"length\"");
    let server = Server::start(vec![
        Reply::text("## Goal\nCompleted history summary"),
        truncated,
    ]);
    let large = "w".repeat(400);
    let tail = "t".repeat(200);
    let conversation = json!([
        { "role": "user", "content": "Earlier turn" },
        { "role": "assistant", "content": "Earlier reply" },
        { "role": "user", "content": format!("Split turn {large}") },
        { "role": "assistant", "content": tail }
    ]);
    let outputs = call_compact(json!({
        "compact.params.messages": conversation,
        "compact.params.force": true,
        "compact.params.keep_recent_tokens": 40,
        "compact.params.model": "fixture-model",
        "compact.params.base_url": server.base_url,
        "compact.params.api_key": "synthetic-key"
    }));
    assert_eq!(outputs["status"].as_str(), Some("failed"));
    assert_eq!(outputs["compacted"].as_bool(), Some(false));
    assert_eq!(
        serde_json::to_value(&outputs["messages"]).unwrap(),
        conversation
    );
    assert!(
        outputs["error"]
            .as_str()
            .unwrap()
            .contains("Turn prefix summarization failed")
    );
    let usages = serde_json::to_value(&outputs["usages"]).unwrap();
    assert_eq!(usages.as_array().unwrap().len(), 2);
    assert_eq!(usages[0]["attribution"]["purpose"], "compaction");
    assert_eq!(usages[1]["attribution"]["purpose"], "compaction");
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn large_transcripts_compact_through_cel_without_size_caps() {
    let server = Server::start(vec![Reply::text("## Goal\nCompacted large transcript")]);
    // Verify transcripts larger than 1 MiB compact cleanly in pure CEL without size caps.
    let huge = "m".repeat(1_200_000);
    let outputs = call_compact(json!({
        "compact.params.messages": [
            { "role": "user", "content": "Inspect huge output" },
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [{ "id": "big-1", "name": "read", "arguments": { "path": "big.log" } }]
            },
            { "role": "tool", "tool_call_id": "big-1", "content": huge },
            { "role": "assistant", "content": "Processed log" }
        ],
        "compact.params.force": true,
        "compact.params.keep_recent_tokens": 0,
        "compact.params.model": "fixture-model",
        "compact.params.base_url": server.base_url,
        "compact.params.api_key": "synthetic-key"
    }));
    assert_eq!(outputs["status"].as_str(), Some("succeeded"));
    assert_eq!(outputs["compacted"].as_bool(), Some(true));
    assert!(outputs["tokens_before"].as_i64().unwrap() > 300_000);
    let sent = request_messages(&server.requests()[0])[1]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(sent.contains("[... 1198000 more characters truncated]"));
}

#[test]
fn chat_host_composes_builtin_compact_into_replace_context_across_reopen() {
    let server = Server::start(vec![
        Reply::text("First answer"),
        Reply::text("Second answer"),
        Reply::text("## Goal\nSummarized earlier turn"),
        Reply::text("Third answer"),
    ]);
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-11-13",
        "dependencies": {},
        "flows": {
            "compact": { "builtin": "mf-chat.chat-compact" }
        },
        "nodes": [
            {
                "id": "params",
                "kind": "builtin.parameters",
                "config": {
                    "parameters": {
                        "messages": { "type": { "list": "object" } },
                        "compact": { "type": "bool", "default": false, "secret": false },
                        "model": { "type": "string", "default": "fixture-model", "secret": false },
                        "base_url": { "type": "string", "default": server.base_url, "secret": false },
                        "api_key": { "type": "string", "default": "synthetic-key" }
                    }
                }
            },
            {
                "id": "model",
                "kind": "builtin.llm",
                "config": {
                    "model": { "$ref": "params.model" },
                    "base_url": { "$ref": "params.base_url" },
                    "api_key": { "$ref": "params.api_key" },
                    "capture_errors": true,
                    "attribution": { "purpose": "conversation" }
                },
                "inputs": {
                    "messages": { "from": { "node": "params", "port": "messages" } }
                }
            },
            {
                "id": "compact",
                "kind": "workflow.call",
                "config": { "flow": "compact" },
                "inputs": {
                    "params.messages": { "from": { "node": "params", "port": "messages" } },
                    "params.append_messages": { "from": { "node": "model", "port": "messages" } },
                    "params.usages": { "from": { "node": "model", "port": "usages" } },
                    "params.enabled": { "from": { "node": "params", "port": "compact" } },
                    "params.force": { "from": { "node": "params", "port": "compact" } },
                    "params.keep_recent_tokens": { "from": { "node": "keep_recent", "port": "value" } },
                    "params.model": { "from": { "node": "params", "port": "model" } },
                    "params.base_url": { "from": { "node": "params", "port": "base_url" } },
                    "params.api_key": { "from": { "node": "params", "port": "api_key" } }
                }
            },
            {
                "id": "keep_recent",
                "kind": "builtin.constant",
                "config": { "value": 8 }
            },
            {
                "id": "route",
                "kind": "builtin.if_else",
                "config": {
                    "branches": [
                        {
                            "id": "replace",
                            "condition": {
                                "source": { "output": "compact.compacted", "path": "" },
                                "operator": "eq",
                                "value": true
                            }
                        }
                    ]
                }
            },
            {
                "id": "combined_usages",
                "kind": "builtin.code",
                "config": {
                    "language": "cel",
                    "inputs": {
                        "left": { "list": "object" },
                        "right": { "list": "object" }
                    },
                    "code": { "value": "left + right" }
                },
                "inputs": {
                    "left": { "from": { "node": "model", "port": "usages" } },
                    "right": { "from": { "node": "compact", "port": "usages" } }
                }
            },
            {
                "id": "normal",
                "kind": "builtin.object",
                "config": {
                    "fields": {
                        "status": "string",
                        "messages": { "list": "object" },
                        "usages": { "list": "object" }
                    }
                },
                "inputs": {
                    "status": { "from": { "node": "model", "port": "status" } },
                    "messages": { "from": { "node": "model", "port": "messages" } },
                    "usages": { "from": { "node": "combined_usages", "port": "value" } }
                }
            },
            {
                "id": "replacement",
                "kind": "builtin.object",
                "config": {
                    "fields": {
                        "status": "string",
                        "context_messages": { "list": "object" },
                        "display_messages": { "list": "object" },
                        "usages": { "list": "object" }
                    }
                },
                "inputs": {
                    "status": { "from": { "node": "model", "port": "status" } },
                    "context_messages": { "from": { "node": "compact", "port": "messages" } },
                    "display_messages": { "from": { "node": "model", "port": "messages" } },
                    "usages": { "from": { "node": "combined_usages", "port": "value" } }
                }
            }
        ],
        "edges": [
            { "from_node": "compact", "to_node": "route" },
            { "from_node": "route", "from_output": "else", "to_node": "normal" },
            { "from_node": "route", "from_output": "replace", "to_node": "replacement" }
        ],
        "outputs": [
            { "name": "append", "node": "normal", "port": "value", "optional": true },
            { "name": "replace_context", "node": "replacement", "port": "value", "optional": true }
        ]
    }))
    .unwrap();

    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("chat.sqlite");
    let open_host = |compact: bool| {
        let profile = WorkflowProfile::new(
            "default",
            definition.clone(),
            WorkflowArguments::try_from(json!({ "params.compact": compact })).unwrap(),
        )
        .unwrap();
        let mut config = HostConfig::new(profile);
        config.automatic_titles = false;
        ChatHost::open(&db_path, config).unwrap()
    };

    let host = open_host(false);
    let session_id = host
        .database()
        .create_session(
            "default",
            Some("System prompt".into()),
            Some("Manual".into()),
        )
        .unwrap()
        .id;
    let first = host
        .submit(&session_id, "First prompt with details")
        .unwrap()
        .join()
        .unwrap();
    assert!(matches!(first.result.update, MessageUpdate::Append { .. }));
    host.close().unwrap();

    let host = open_host(true);
    let second = host
        .submit(&session_id, "Second prompt kept")
        .unwrap()
        .join()
        .unwrap();
    assert!(matches!(
        second.result.update,
        MessageUpdate::ReplaceContext { .. }
    ));
    assert_eq!(second.result.usages.len(), 2);
    assert_eq!(
        second.result.usages[1]
            .attribution
            .as_ref()
            .unwrap()
            .purpose
            .as_deref(),
        Some("compaction")
    );
    host.close().unwrap();

    // Reopen and send a third turn: the provider must receive system + summary + kept turn + third user.
    let host = open_host(false);
    host.submit(&session_id, "Third prompt")
        .unwrap()
        .join()
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    let third_request = request_messages(&requests[3]);
    assert_eq!(third_request.len(), 5);
    assert_eq!(third_request[0]["role"], "system");
    assert_eq!(third_request[0]["content"], "System prompt");
    assert_eq!(third_request[1]["role"], "user");
    assert!(
        third_request[1]["content"]
            .as_str()
            .unwrap()
            .contains("<summary>\n## Goal\nSummarized earlier turn\n</summary>")
    );
    assert_eq!(third_request[2]["content"], "Second prompt kept");
    assert_eq!(third_request[3]["content"], "Second answer");
    assert_eq!(third_request[4]["content"], "Third prompt");

    // Full display transcript retains all 6 original messages untouched.
    let transcript = host
        .database()
        .messages(&session_id, 50, None)
        .unwrap()
        .messages;
    assert_eq!(transcript.len(), 6);
    assert_eq!(transcript[0].message.content(), "First prompt with details");
    assert_eq!(transcript[1].message.content(), "First answer");
    assert_eq!(transcript[5].message.content(), "Third answer");
    host.close().unwrap();
}

#[test]
fn disabled_fast_path_preserves_secret_messages_without_tainting_metadata() {
    let secret_messages = ValueRef::array([
        ValueRef::object([
            ("role".into(), "user".into()),
            ("content".into(), ValueRef::from("secret").into_secret()),
        ]),
        ValueRef::object([
            ("role".into(), "assistant".into()),
            ("content".into(), "public reply".into()),
        ]),
    ]);
    let mut args = WorkflowArguments::default();
    args.0
        .insert("compact.params.messages".into(), secret_messages);
    args.0.insert("compact.params.enabled".into(), false.into());
    args.0
        .insert("compact.params.api_key".into(), "synthetic-key".into());

    let outputs = try_call_compact(args).unwrap();
    assert_eq!(outputs["status"].as_str(), Some("skipped"));
    assert_eq!(outputs["compacted"].as_bool(), Some(false));
    assert_eq!(outputs["tokens_before"].as_i64(), Some(0));
    assert!(outputs["messages"].is_secret());
    for public_output in [
        "status",
        "compacted",
        "summary",
        "usages",
        "tokens_before",
        "read_files",
        "modified_files",
        "error",
    ] {
        assert!(
            !outputs[public_output].is_secret(),
            "disabled fast path should not taint `{public_output}`"
        );
    }
}

#[test]
fn rejects_negative_compaction_settings() {
    for setting in [
        "compact.params.context_window",
        "compact.params.reserve_tokens",
        "compact.params.keep_recent_tokens",
    ] {
        let mut args = json!({
            "compact.params.messages": [
                { "role": "user", "content": "Hello" },
                { "role": "assistant", "content": "Hi" }
            ],
            "compact.params.enabled": false,
            "compact.params.api_key": "synthetic-key"
        });
        args[setting] = json!(-1);
        let error = try_call_compact(WorkflowArguments::try_from(args).unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("non-negative"), "{setting}: {error}");
    }
}

#[test]
fn detects_legacy_summary_wrapper_and_preserves_previous_summary_on_split_turn() {
    let server = Server::start(vec![Reply::text("## Original Request\nSplit after legacy")]);
    let legacy_summary = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n## Goal\nLegacy summary\n</summary>";
    let large = "z".repeat(400);
    let outputs = call_compact(json!({
        "compact.params.messages": [
            { "role": "user", "content": legacy_summary },
            { "role": "user", "content": format!("Long turn {large}") },
            { "role": "assistant", "content": "Kept assistant tail" }
        ],
        "compact.params.force": true,
        "compact.params.keep_recent_tokens": 4,
        "compact.params.model": "fixture-model",
        "compact.params.base_url": server.base_url,
        "compact.params.api_key": "synthetic-key"
    }));
    assert_eq!(outputs["status"].as_str(), Some("succeeded"));
    assert_eq!(server.requests().len(), 1);
    let summary = outputs["summary"].as_str().unwrap();
    assert!(
        summary.starts_with("## Goal\nLegacy summary\n\n---\n\n**Turn Context (split turn):**"),
        "unexpected summary: {summary}"
    );
    assert!(summary.contains("## Original Request\nSplit after legacy"));
}

#[test]
fn cut_point_never_starts_kept_messages_on_tool_result_and_usage_filters_auxiliary_calls() {
    let server = Server::start(vec![Reply::text("## Goal\nSummarized before tool pair")]);
    let huge_tool = "t".repeat(400);
    // Keep-recent lands on the tool result at index 3; cut must snap back to the last valid
    // non-tool cut point (assistant at index 2) so the kept suffix stays a valid tool pair.
    let outputs = call_compact(json!({
        "compact.params.messages": [
            { "role": "user", "content": "Earlier turn" },
            { "role": "assistant", "content": "Earlier answer" },
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [{ "id": "call-1", "name": "lookup", "arguments": ["raw-arg"] }]
            },
            { "role": "tool", "tool_call_id": "call-1", "content": huge_tool }
        ],
        "compact.params.usages": [
            {
                "model": "fixture-model",
                "attribution": { "purpose": "conversation" },
                "usage": { "input_tokens": 400, "output_tokens": 100, "cached_input_tokens": 50 }
            },
            {
                "model": "fixture-model",
                "attribution": { "purpose": "title" },
                "usage": { "total_tokens": 9999 }
            },
            {
                "model": "fixture-model",
                "attribution": { "purpose": "compaction" },
                "usage": { "total_tokens": 8888 }
            },
            {
                "model": "fixture-model",
                "error": "ignored error usage",
                "usage": { "total_tokens": 7777 }
            }
        ],
        "compact.params.context_window": 700,
        "compact.params.reserve_tokens": 100,
        "compact.params.keep_recent_tokens": 50,
        "compact.params.model": "fixture-model",
        "compact.params.base_url": server.base_url,
        "compact.params.api_key": "synthetic-key"
    }));
    // 400 + 100 + 50 from conversation usage + 100 estimated tokens for trailing tool result (400 / 4).
    assert_eq!(outputs["tokens_before"].as_i64(), Some(650));
    assert_eq!(outputs["status"].as_str(), Some("succeeded"));
    let rebuilt = serde_json::to_value(&outputs["messages"]).unwrap();
    let rebuilt = rebuilt.as_array().unwrap();
    assert_eq!(rebuilt.len(), 3);
    assert_eq!(rebuilt[0]["role"], "user");
    assert_eq!(rebuilt[1]["role"], "assistant");
    assert_eq!(rebuilt[2]["role"], "tool");
}
