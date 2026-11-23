use mf_chat::{
    ChatError, ChatHost, CompactionConfig, ConversationBackend, ConversationInputs,
    ConversationResult, HostConfig, Invocation, MessageUpdate, WorkflowProfile,
};
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{ExecutionContext, NodeValues, ValueRef, WorkflowArguments};
use mf_telemetry::observation::Observer;
use mfn_llm::test_support::{Reply, Server, messages as request_messages};
use serde_json::{Value, json};
use std::time::Duration;

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
        (
            "disabled",
            json!({ "compact.params.context": { "compaction": { "enabled": false } } }),
        ),
        (
            "below_threshold",
            json!({
                "compact.params.context": {
                    "compaction": {
                        "context_window": 10000,
                        "reserve_tokens": 1000
                    }
                }
            }),
        ),
        (
            "fits_keep_recent",
            json!({
                "compact.params.force": true,
                "compact.params.context": {
                    "compaction": {
                        "keep_recent_tokens": 20000
                    }
                }
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
        "compact.params.context": {
            "compaction": {
                "context_window": 1000,
                "reserve_tokens": 200,
                "keep_recent_tokens": 12,
                "custom_instructions": "Preserve file paths."
            }
        },
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
        "compact.params.context": {
            "compaction": {
                "enabled": false,
                "keep_recent_tokens": 50
            }
        },
        "compact.params.force": true,
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
        "compact.params.context": { "compaction": { "keep_recent_tokens": 4 } },
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
        "compact.params.context": { "compaction": { "keep_recent_tokens": 0 } },
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
        "compact.params.context": { "compaction": { "keep_recent_tokens": 0 } },
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
        "compact.params.context": { "compaction": { "keep_recent_tokens": 0 } },
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
            "compact.params.context": { "compaction": { "keep_recent_tokens": 40 } },
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
        "compact.params.context": { "compaction": { "keep_recent_tokens": 40 } },
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
        "compact.params.context": { "compaction": { "keep_recent_tokens": 0 } },
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
                "id": "compact_context",
                "kind": "builtin.code",
                "config": {
                    "language": "cel",
                    "inputs": { "enabled": "bool" },
                    "code": {
                        "value": "{'compaction': {'enabled': enabled, 'keep_recent_tokens': 8}}"
                    }
                },
                "inputs": {
                    "enabled": { "from": { "node": "params", "port": "compact" } }
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
                    "params.context": { "from": { "node": "compact_context", "port": "value" } },
                    "params.force": { "from": { "node": "params", "port": "compact" } },
                    "params.model": { "from": { "node": "params", "port": "model" } },
                    "params.base_url": { "from": { "node": "params", "port": "base_url" } },
                    "params.api_key": { "from": { "node": "params", "port": "api_key" } }
                }
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
    args.0.insert(
        "compact.params.context".into(),
        ValueRef::object([(
            "compaction".into(),
            ValueRef::object([("enabled".into(), false.into())]),
        )]),
    );
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
    for setting in ["context_window", "reserve_tokens", "keep_recent_tokens"] {
        let args = json!({
            "compact.params.messages": [
                { "role": "user", "content": "Hello" },
                { "role": "assistant", "content": "Hi" }
            ],
            "compact.params.context": {
                "compaction": {
                    "enabled": false,
                    setting: -1
                }
            },
            "compact.params.api_key": "synthetic-key"
        });
        let error = try_call_compact(WorkflowArguments::try_from(args).unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("non-negative"), "{setting}: {error}");
    }
    let bad_type = json!({
        "compact.params.messages": [
            { "role": "user", "content": "Hello" },
            { "role": "assistant", "content": "Hi" }
        ],
        "compact.params.context": {
            "compaction": "invalid"
        },
        "compact.params.api_key": "synthetic-key"
    });
    assert!(try_call_compact(WorkflowArguments::try_from(bad_type).unwrap()).is_err());
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
        "compact.params.context": { "compaction": { "keep_recent_tokens": 4 } },
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
        "compact.params.context": {
            "compaction": {
                "context_window": 700,
                "reserve_tokens": 100,
                "keep_recent_tokens": 50
            }
        },
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

#[test]
fn compact_and_helper_flows_declare_annotated_clusters_covering_all_nodes() {
    let registry = mf_runtime::BuiltinFlowRegistry::from_inventory().unwrap();
    for flow_name in [
        "mf-chat.chat-compact",
        "mf-chat.chat-compact-run",
        "mf-chat.chat-compact-plan",
        "mf-chat.chat-compact-summarize",
    ] {
        let definition = (registry
            .get(flow_name)
            .unwrap_or_else(|| panic!("missing builtin flow {flow_name}"))
            .definition)()
        .unwrap();
        assert!(
            !definition.clusters.is_empty(),
            "{flow_name} should organize nodes into clusters"
        );
        let mut covered = std::collections::BTreeSet::new();
        for cluster in &definition.clusters {
            assert!(
                !cluster.annotation.trim().is_empty(),
                "{flow_name} cluster `{}` must carry a non-empty annotation",
                cluster.label
            );
            for node_id in &cluster.nodes {
                assert!(
                    covered.insert(node_id.clone()),
                    "{flow_name} assigns node `{node_id}` to multiple clusters"
                );
            }
        }
        let all_nodes: std::collections::BTreeSet<_> = definition
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect();
        assert_eq!(
            covered, all_nodes,
            "{flow_name} clusters should cover every node exactly once"
        );
    }
}

fn plain_chat_host(
    directory: &std::path::Path,
    server: &Server,
    automatic_titles: bool,
    automatic_compaction: CompactionConfig,
) -> ChatHost {
    let definition =
        WorkflowDefinition::from_json(include_str!("../../../examples/chat-flow.json")).unwrap();
    let profile = WorkflowProfile::new(
        "default",
        definition,
        WorkflowArguments::try_from(json!({
            "params.model": "fixture-model",
            "params.base_url": server.base_url,
            "params.api_key": "synthetic-key"
        }))
        .unwrap(),
    )
    .unwrap();
    let mut config = HostConfig::new(profile);
    config.automatic_titles = automatic_titles;
    config.automatic_compaction = automatic_compaction;
    ChatHost::open(directory.join("chat.sqlite"), config).unwrap()
}

fn usage_reply(text: &str, total_tokens: i64) -> Reply {
    Reply::json(json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "model": "fixture-model",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": text },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": total_tokens - 10,
            "completion_tokens": 10,
            "total_tokens": total_tokens
        }
    }))
}

#[test]
fn host_builtin_compaction_triggers_automatically_and_persists_across_reopen() {
    let server = Server::start(vec![
        usage_reply("First answer", 40),
        Reply::text("Project Alpha"),
        usage_reply("Second answer", 95),
        Reply::text("## Goal\nSummarized turn one"),
        usage_reply("Third answer", 35),
    ]);
    let root = tempfile::tempdir().unwrap();
    let compaction = CompactionConfig {
        enabled: true,
        context_window: Some(100),
        reserve_tokens: 20,
        keep_recent_tokens: 12,
        custom_instructions: "Preserve project names.".into(),
    };
    let host = plain_chat_host(root.path(), &server, true, compaction.clone());
    let session_id = host
        .database()
        .create_session("default", Some("System prompt".into()), None)
        .unwrap()
        .id;

    // Turn 1: under threshold (40 <= 80). Host generates title and skips compaction.
    let first = host
        .submit(&session_id, "Discuss Project Alpha details")
        .unwrap()
        .join()
        .unwrap();
    assert!(matches!(first.result.update, MessageUpdate::Append { .. }));
    assert_eq!(first.result.title.as_deref(), Some("Project Alpha"));
    assert_eq!(first.result.usages.len(), 2);
    assert_eq!(server.requests().len(), 2);

    // Turn 2: over threshold (95 > 80). Host automatically runs mf-chat.chat-compact.
    let second = host
        .submit(&session_id, "Follow up on Project Alpha")
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(second.result.final_text(), Some("Second answer"));
    assert!(matches!(
        second.result.update,
        MessageUpdate::ReplaceContext { .. }
    ));
    assert_eq!(second.result.usages.len(), 2);
    let compact_usage = &second.result.usages[1];
    assert_eq!(
        compact_usage
            .attribution
            .as_ref()
            .and_then(|attribution| attribution.purpose.as_deref()),
        Some("compaction")
    );
    assert!(
        compact_usage
            .timing
            .as_ref()
            .is_some_and(|timing| timing.is_valid())
    );
    let compact_request = &server.requests()[3];
    let compact_messages = request_messages(compact_request);
    let compact_prompt = compact_messages[1]["content"].as_str().unwrap();
    assert!(compact_prompt.contains("Preserve project names."));
    host.close().unwrap();

    // Reopen and verify the compacted context is sent to Turn 3 while full display history remains.
    let reopened = plain_chat_host(root.path(), &server, true, compaction);
    let context_before_turn3 = reopened.database().model_context(&session_id).unwrap();
    assert!(context_before_turn3.messages.iter().any(|message| matches!(
        message,
        mfn_llm::ChatMessage::User { content, metadata: Some(meta) }
        if content.contains("## Goal\nSummarized turn one") && meta.get("compaction").is_some()
    )));
    let third = reopened
        .submit(&session_id, "Third question")
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(third.result.final_text(), Some("Third answer"));
    assert!(matches!(third.result.update, MessageUpdate::Append { .. }));

    let turn3_messages = request_messages(&server.requests()[4]);
    assert!(turn3_messages.iter().any(|message| {
        message["content"]
            .as_str()
            .is_some_and(|text| text.contains("Summarized turn one"))
    }));
    assert!(!turn3_messages.iter().any(|message| {
        message["content"]
            .as_str()
            .is_some_and(|text| text.contains("Discuss Project Alpha details"))
    }));
    assert_eq!(
        reopened
            .database()
            .messages(&session_id, 50, None)
            .unwrap()
            .messages
            .len(),
        6
    );
    reopened.close().unwrap();
}

#[test]
fn host_builtin_compaction_failure_preserves_answer_and_records_failed_usage() {
    let mut failed_compact = Reply::json(json!({"error":{"message":"summary unavailable"}}));
    failed_compact.status = 500;
    let server = Server::start(vec![
        usage_reply("First answer", 40),
        usage_reply("Second answer", 95),
        failed_compact,
    ]);
    let root = tempfile::tempdir().unwrap();
    let host = plain_chat_host(
        root.path(),
        &server,
        false,
        CompactionConfig {
            enabled: true,
            context_window: Some(100),
            reserve_tokens: 20,
            keep_recent_tokens: 4,
            custom_instructions: String::new(),
        },
    );
    let session_id = host
        .database()
        .create_session("default", None, Some("Manual".into()))
        .unwrap()
        .id;

    host.submit(&session_id, "Turn one details to summarize")
        .unwrap()
        .join()
        .unwrap();
    let second = host
        .submit(&session_id, "Turn two")
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(second.result.final_text(), Some("Second answer"));
    assert!(matches!(second.result.update, MessageUpdate::Append { .. }));
    assert_eq!(second.result.usages.len(), 2);
    let compact_usage = &second.result.usages[1];
    assert_eq!(
        compact_usage
            .attribution
            .as_ref()
            .and_then(|attribution| attribution.purpose.as_deref()),
        Some("compaction")
    );
    assert!(compact_usage.error.is_some());
    assert_eq!(
        host.database()
            .model_context(&session_id)
            .unwrap()
            .messages
            .len(),
        4
    );
    host.close().unwrap();
}

#[test]
fn host_builtin_compaction_respects_disabled_config_failed_answers_and_validates_settings() {
    let server = Server::start(vec![]);
    let root = tempfile::tempdir().unwrap();
    let definition =
        WorkflowDefinition::from_json(include_str!("../../../examples/chat-flow.json")).unwrap();
    let profile = WorkflowProfile::new(
        "default",
        definition,
        WorkflowArguments::try_from(json!({
            "params.model": "fixture-model",
            "params.base_url": server.base_url,
            "params.api_key": "synthetic-key"
        }))
        .unwrap(),
    )
    .unwrap();
    let mut bad_config = HostConfig::new(profile);
    bad_config.automatic_compaction.reserve_tokens = -1;
    assert!(matches!(
        ChatHost::open(root.path().join("invalid.sqlite"), bad_config),
        Err(ChatError::Invalid {
            field: "automatic_compaction",
            ..
        })
    ));

    for scenario in ["disabled", "failed"] {
        let reply = if scenario == "failed" {
            let mut failed = Reply::json(json!({"error":{"message":"answer failed"}}));
            failed.status = 400;
            failed
        } else {
            usage_reply("Answer", 95)
        };
        let server = Server::start(vec![reply]);
        let case_root = tempfile::tempdir().unwrap();
        let host = plain_chat_host(
            case_root.path(),
            &server,
            false,
            CompactionConfig {
                enabled: scenario != "disabled",
                context_window: Some(100),
                reserve_tokens: 20,
                keep_recent_tokens: 0,
                custom_instructions: String::new(),
            },
        );
        let session_id = host
            .database()
            .create_session("default", None, Some("Manual".into()))
            .unwrap()
            .id;
        let outcome = host.submit(&session_id, "Prompt").unwrap().join();
        assert_eq!(outcome.is_err(), scenario == "failed");
        assert_eq!(server.requests().len(), 1);
        host.close().unwrap();
    }
}

#[test]
fn cancelling_builtin_compaction_does_not_commit_an_intermediate_answer() {
    let mut slow_compact = Reply::text("## Goal\nSlow summary");
    slow_compact.header_delay = Duration::from_millis(1200);
    let server = Server::start(vec![
        usage_reply("First answer", 40),
        usage_reply("Second answer", 95),
        slow_compact,
    ]);
    let root = tempfile::tempdir().unwrap();
    let host = plain_chat_host(
        root.path(),
        &server,
        false,
        CompactionConfig {
            enabled: true,
            context_window: Some(100),
            reserve_tokens: 20,
            keep_recent_tokens: 4,
            custom_instructions: String::new(),
        },
    );
    let session_id = host
        .database()
        .create_session("default", None, Some("Manual".into()))
        .unwrap()
        .id;
    host.submit(&session_id, "Turn one details to summarize")
        .unwrap()
        .join()
        .unwrap();

    let job = host.submit(&session_id, "Turn two").unwrap();
    let invocation = job.invocation().clone();
    server.wait_for_requests(3);
    job.cancel();
    assert!(job.join().is_err());
    assert_eq!(
        host.database().turn(&invocation.turn_id).unwrap().state,
        "cancelled"
    );
    assert_eq!(
        host.database()
            .model_context(&session_id)
            .unwrap()
            .messages
            .len(),
        2
    );
    host.close().unwrap();
}

#[test]
fn conversation_finish_retains_compaction_for_save_only_retry() {
    use diesel::{Connection, connection::SimpleConnection};
    let server = Server::start(vec![
        usage_reply("First answer", 40),
        usage_reply("Second answer", 95),
        Reply::text("## Goal\nRecoverable summary"),
    ]);
    let root = tempfile::tempdir().unwrap();
    let host = plain_chat_host(
        root.path(),
        &server,
        false,
        CompactionConfig {
            enabled: true,
            context_window: Some(100),
            reserve_tokens: 20,
            keep_recent_tokens: 6,
            custom_instructions: String::new(),
        },
    );
    let session_id = host
        .database()
        .create_session("default", None, Some("Manual".into()))
        .unwrap()
        .id;
    host.submit(&session_id, "Turn one details to summarize")
        .unwrap()
        .join()
        .unwrap();

    let mut connection =
        diesel::SqliteConnection::establish(host.database().path().to_str().unwrap()).unwrap();
    connection
        .batch_execute(
            "CREATE TRIGGER reject_replacement BEFORE INSERT ON context_revisions WHEN NEW.operation = 'checkpoint' BEGIN SELECT RAISE(ABORT, 'injected compaction write failure'); END;",
        )
        .unwrap();
    let error = host
        .submit(&session_id, "Turn two")
        .unwrap()
        .join()
        .unwrap_err();
    let ChatError::Uncommitted { pending, .. } = error else {
        panic!("expected compacted completion to remain recoverable");
    };
    assert!(matches!(
        pending.completion.result.update,
        MessageUpdate::ReplaceContext { .. }
    ));
    assert_eq!(pending.completion.result.usages.len(), 2);
    assert_eq!(
        host.database()
            .model_context(&session_id)
            .unwrap()
            .messages
            .len(),
        2
    );

    connection
        .batch_execute("DROP TRIGGER reject_replacement;")
        .unwrap();
    let saved = host.retry_completion(*pending).unwrap();
    assert_eq!(saved.result.final_text(), Some("Second answer"));
    assert!(matches!(
        saved.result.update,
        MessageUpdate::ReplaceContext { .. }
    ));
    assert!(
        host.database()
            .model_context(&session_id)
            .unwrap()
            .messages
            .iter()
            .any(|message| matches!(
                message,
                mfn_llm::ChatMessage::User { content, .. } if content.contains("Recoverable summary")
            ))
    );
    assert_eq!(server.requests().len(), 3);
    host.close().unwrap();
}

#[test]
fn chatflow_reusing_builtin_compact_receives_host_policy_and_suppresses_host_compaction() {
    let server = Server::start(vec![
        usage_reply("First answer", 40),
        usage_reply("Second answer", 95),
        Reply::text("## Goal\nUser-flow reused compaction"),
    ]);
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-11-13",
        "dependencies": {
            "core": { "package": "mfn-core", "path": "../../crates/builtin-nodes/core" },
            "code": { "package": "mfn-code", "path": "../../crates/builtin-nodes/code" },
            "llm": { "package": "mfn-llm", "path": "../../crates/builtin-nodes/llm" }
        },
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
                        "context": {
                            "type": "object",
                            "default": {
                                "title": { "source": "provisional", "revision": 0, "attempted": false, "enabled": true },
                                "model_context": { "revision": 0 },
                                "compaction": { "enabled": true, "context_window": 128000, "reserve_tokens": 16384, "keep_recent_tokens": 20000, "custom_instructions": "" }
                            }
                        },
                        "model": { "type": "string", "default": "fixture-model", "secret": false },
                        "base_url": { "type": "string", "default": "https://api.openai.com/v1", "secret": false },
                        "api_key": { "type": "string", "default": "synthetic-key", "secret": true }
                    }
                }
            },
            {
                "id": "chat",
                "kind": "builtin.llm",
                "config": {
                    "model": { "$ref": "params.model" },
                    "base_url": { "$ref": "params.base_url" },
                    "api_key": { "$ref": "params.api_key" },
                    "capture_errors": true
                },
                "inputs": {
                    "messages": { "from": { "node": "params", "port": "messages" } }
                }
            },
            {
                "id": "turn",
                "kind": "builtin.code",
                "config": {
                    "language": "cel",
                    "inputs": {
                        "text": "string"
                    },
                    "code": {
                        "append_messages": "[{'role': 'assistant', 'content': text}]"
                    }
                },
                "inputs": {
                    "text": { "from": { "node": "chat", "port": "text" } }
                }
            },
            {
                "id": "compact",
                "kind": "workflow.call",
                "config": { "flow": "compact" },
                "inputs": {
                    "params.messages": { "from": { "node": "params", "port": "messages" } },
                    "params.append_messages": { "from": { "node": "turn", "port": "append_messages" } },
                    "params.usages": { "from": { "node": "chat", "port": "usages" } },
                    "params.context": { "from": { "node": "params", "port": "context" } },
                    "params.model": { "from": { "node": "params", "port": "model" } },
                    "params.base_url": { "from": { "node": "params", "port": "base_url" } },
                    "params.api_key": { "from": { "node": "params", "port": "api_key" } }
                }
            },
            {
                "id": "route",
                "kind": "builtin.if_else",
                "config": {
                    "branches": [{
                        "id": "replaced",
                        "condition": {
                            "source": { "output": "compact.compacted", "path": "" },
                            "operator": "eq",
                            "value": true
                        }
                    }]
                }
            },
            {
                "id": "append_result",
                "kind": "builtin.code",
                "config": {
                    "language": "cel",
                    "inputs": {
                        "append_messages": { "list": "object" },
                        "turn_usages": { "list": "object" },
                        "compact_usages": { "list": "object" }
                    },
                    "code": {
                        "value": "{'status': 'succeeded', 'messages': append_messages, 'usages': turn_usages + compact_usages}"
                    }
                },
                "inputs": {
                    "append_messages": { "from": { "node": "turn", "port": "append_messages" } },
                    "turn_usages": { "from": { "node": "chat", "port": "usages" } },
                    "compact_usages": { "from": { "node": "compact", "port": "usages" } }
                }
            },
            {
                "id": "replace_result",
                "kind": "builtin.code",
                "config": {
                    "language": "cel",
                    "inputs": {
                        "append_messages": { "list": "object" },
                        "turn_usages": { "list": "object" },
                        "context_messages": { "list": "object" },
                        "compact_usages": { "list": "object" }
                    },
                    "code": {
                        "value": "{'status': 'succeeded', 'context_messages': context_messages, 'display_messages': append_messages, 'usages': turn_usages + compact_usages}"
                    }
                },
                "inputs": {
                    "append_messages": { "from": { "node": "turn", "port": "append_messages" } },
                    "turn_usages": { "from": { "node": "chat", "port": "usages" } },
                    "context_messages": { "from": { "node": "compact", "port": "messages" } },
                    "compact_usages": { "from": { "node": "compact", "port": "usages" } }
                }
            }
        ],
        "edges": [
            { "from_node": "compact", "to_node": "route" },
            { "from_node": "route", "from_output": "replaced", "to_node": "replace_result" },
            { "from_node": "route", "from_output": "else", "to_node": "append_result" }
        ],
        "outputs": [
            { "name": "append", "node": "append_result", "port": "value", "optional": true },
            { "name": "replace_context", "node": "replace_result", "port": "value", "optional": true }
        ]
    }))
    .unwrap();

    let profile = WorkflowProfile::new(
        "default",
        definition,
        WorkflowArguments::try_from(json!({
            "params.model": "fixture-model",
            "params.base_url": server.base_url,
            "params.api_key": "synthetic-key"
        }))
        .unwrap(),
    )
    .unwrap();
    let mut config = HostConfig::new(profile);
    config.automatic_titles = false;
    config.automatic_compaction = CompactionConfig {
        enabled: true,
        context_window: Some(100),
        reserve_tokens: 20,
        keep_recent_tokens: 6,
        custom_instructions: "Follow host custom instructions.".into(),
    };
    let root = tempfile::tempdir().unwrap();
    let host = ChatHost::open(root.path().join("chat.sqlite"), config).unwrap();
    let session_id = host
        .database()
        .create_session("default", None, Some("Manual".into()))
        .unwrap()
        .id;

    // Turn 1: under threshold -> returns append; host does NOT run a second compaction.
    let first = host
        .submit(&session_id, "Turn one details to summarize")
        .unwrap()
        .join()
        .unwrap();
    assert!(matches!(first.result.update, MessageUpdate::Append { .. }));
    assert_eq!(server.requests().len(), 1);

    // Turn 2: over threshold -> user chatflow runs mf-chat.chat-compact once; host yields.
    let second = host
        .submit(&session_id, "Turn two")
        .unwrap()
        .join()
        .unwrap();
    assert!(matches!(
        second.result.update,
        MessageUpdate::ReplaceContext { .. }
    ));
    assert_eq!(second.result.usages.len(), 2);
    assert_eq!(server.requests().len(), 3);
    let compact_messages = request_messages(&server.requests()[2]);
    let compact_prompt = compact_messages[1]["content"].as_str().unwrap();
    assert!(compact_prompt.contains("Follow host custom instructions."));
    host.close().unwrap();
}

struct CustomCompactingBackend {
    base_url: String,
    report_compaction_usage: bool,
}

impl ConversationBackend for CustomCompactingBackend {
    fn builtin_title_parameters(&self) -> Option<WorkflowArguments> {
        Some(
            WorkflowArguments::try_from(json!({
                "params.model": "fixture-model",
                "params.base_url": self.base_url,
                "params.api_key": "synthetic-key"
            }))
            .unwrap(),
        )
    }

    fn invoke(
        &self,
        _: &Invocation,
        input: ConversationInputs,
        _: &mut ExecutionContext,
        _: Option<&mut dyn FnMut(mf_chat::PreviewDelta) -> Result<(), ChatError>>,
        _: Option<&Observer>,
    ) -> Result<ConversationResult, ChatError> {
        let context = input.context.expect("host passes compaction context");
        assert!(context.compaction.enabled);
        let mut result = ConversationResult::assistant("Custom backend answer");
        if self.report_compaction_usage {
            result.usages.push(mf_chat::CallUsage {
                started_at: Some(0),
                call_id: "custom-compact-1".into(),
                node: None,
                model: "fixture-model".into(),
                usage: None,
                attribution: Some(mfn_llm::UsageAttribution {
                    purpose: Some("compaction".into()),
                    profile: None,
                }),
                timing: None,
                error: Some("custom compaction skipped".into()),
            });
        } else {
            result.update = MessageUpdate::ReplaceContext {
                context_messages: vec![mfn_llm::ChatMessage::System {
                    content: "Custom compacted context".into(),
                    metadata: None,
                }],
                display_messages: vec![mfn_llm::ChatMessage::Assistant {
                    content: "Custom backend answer".into(),
                    tool_calls: None,
                    metadata: None,
                }],
            };
        }
        Ok(result)
    }
}

#[test]
fn custom_compaction_result_or_compaction_usage_suppresses_host_compaction() {
    for report_compaction_usage in [false, true] {
        let server = Server::start(vec![]);
        let profile = WorkflowProfile::with_backend(
            "custom",
            std::sync::Arc::new(CustomCompactingBackend {
                base_url: server.base_url.clone(),
                report_compaction_usage,
            }),
        )
        .unwrap();
        let mut config = HostConfig::new(profile);
        config.automatic_titles = false;
        config.automatic_compaction = CompactionConfig {
            enabled: true,
            context_window: Some(10),
            reserve_tokens: 5,
            keep_recent_tokens: 0,
            custom_instructions: String::new(),
        };
        let root = tempfile::tempdir().unwrap();
        let host = ChatHost::open(root.path().join("chat.sqlite"), config).unwrap();
        let session_id = host
            .database()
            .create_session("custom", None, Some("Manual".into()))
            .unwrap()
            .id;

        let completion = host
            .submit(&session_id, "Prompt well above host compaction threshold")
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(
            completion.result.final_text(),
            Some("Custom backend answer")
        );
        assert_eq!(
            server.requests().len(),
            0,
            "host must not invoke builtin compaction when custom backend handled compaction"
        );
        host.close().unwrap();
    }
}

#[test]
fn embedded_conversation_declares_and_prepares_builtin_compact_call() {
    let authored =
        WorkflowDefinition::from_json(include_str!("../workflows/conversation.json")).unwrap();
    assert_eq!(
        authored.flows["compact"],
        mf_runtime::FlowSource::Builtin {
            builtin: "mf-chat.chat-compact".into()
        }
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = mf_chat::flows::conversation::plan(&registry).unwrap();
    for (id, kind) in [
        ("compact_gate", "builtin.code"),
        ("finish", mf_runtime::CALL_KIND),
    ] {
        assert!(
            plan.graph
                .definition
                .nodes
                .iter()
                .any(|node| node.id.as_str() == id && node.kind == kind),
            "missing node {id} ({kind})"
        );
    }
    let call = plan
        .graph
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "compact")
        .unwrap();
    assert_eq!(call.kind, mf_runtime::CALL_KIND);
    let configuration: mf_runtime::CallConfig =
        serde_json::from_value(call.config.clone()).unwrap();
    let body = configuration.body.unwrap();
    let registered = mf_chat::flows::compact_definition().unwrap();
    let resolved = mf_compiler::resolve_flows_with_registry(&registered, None, &registry).unwrap();
    let mut expected = mf_compiler::plan_resolved_workflow(&resolved)
        .unwrap()
        .graph
        .definition;
    // Standalone resolution hoists descendant dependencies onto its own root, whereas nested call
    // bodies retain only their direct dependencies.
    expected.dependencies = registered.dependencies;
    assert_eq!(*body, expected);
}

#[test]
fn host_discovers_and_caches_model_context_window_for_builtin_and_reused_compaction() {
    // 1. Host-managed automatic compaction discovers context_window via /models on turn 1
    //    and reuses the cached window to trigger compaction on turn 2.
    let server = Server::start_with_models(
        vec![
            usage_reply("First turn answer", 40),
            usage_reply("Second turn answer", 90),
            Reply::text("## Goal\nDiscovered window summary"),
        ],
        vec![Reply::models(json!([
            {"id": "fixture-model", "context_length": 100}
        ]))],
    );
    let root = tempfile::tempdir().unwrap();
    let host = plain_chat_host(
        root.path(),
        &server,
        false,
        CompactionConfig {
            enabled: true,
            context_window: None,
            reserve_tokens: 20,
            keep_recent_tokens: 6,
            custom_instructions: String::new(),
        },
    );
    let session_id = host
        .database()
        .create_session("default", None, Some("Manual".into()))
        .unwrap()
        .id;
    let first = host
        .submit(&session_id, "First turn details to summarize")
        .unwrap()
        .join()
        .unwrap();
    assert!(matches!(first.result.update, MessageUpdate::Append { .. }));
    let second = host
        .submit(&session_id, "Second turn over discovered window")
        .unwrap()
        .join()
        .unwrap();
    assert!(matches!(
        second.result.update,
        MessageUpdate::ReplaceContext { .. }
    ));
    assert_eq!(
        server.model_requests().len(),
        1,
        "context_window probe must be cached across turns on the same ChatHost"
    );
    host.close().unwrap();

    // 2. Explicit context_window override skips /models entirely.
    let explicit_server = Server::start_with_models(
        vec![usage_reply("Explicit answer", 30)],
        vec![Reply::models(json!([
            {"id": "fixture-model", "context_window": 10}
        ]))],
    );
    let explicit_root = tempfile::tempdir().unwrap();
    let explicit_host = plain_chat_host(
        explicit_root.path(),
        &explicit_server,
        false,
        CompactionConfig {
            enabled: true,
            context_window: Some(1_000),
            reserve_tokens: 20,
            keep_recent_tokens: 6,
            custom_instructions: String::new(),
        },
    );
    let explicit_session = explicit_host
        .database()
        .create_session("default", None, Some("Manual".into()))
        .unwrap()
        .id;
    let explicit_turn = explicit_host
        .submit(&explicit_session, "Explicit override turn")
        .unwrap()
        .join()
        .unwrap();
    assert!(matches!(
        explicit_turn.result.update,
        MessageUpdate::Append { .. }
    ));
    assert!(
        explicit_server.model_requests().is_empty(),
        "explicit context_window must not issue a /models request"
    );
    explicit_host.close().unwrap();

    // 3. Missing/failed /models falls back to DEFAULT_COMPACTION_CONTEXT_WINDOW and caches the miss.
    let mut forbidden = Reply::json(json!({"error": {"message": "models disabled"}}));
    forbidden.status = 403;
    let fallback_server = Server::start_with_models(
        vec![
            usage_reply("Fallback turn 1", 90),
            usage_reply("Fallback turn 2", 90),
        ],
        vec![forbidden],
    );
    let fallback_root = tempfile::tempdir().unwrap();
    let fallback_host = plain_chat_host(
        fallback_root.path(),
        &fallback_server,
        false,
        CompactionConfig {
            enabled: true,
            context_window: None,
            reserve_tokens: 20,
            keep_recent_tokens: 6,
            custom_instructions: String::new(),
        },
    );
    let fallback_session = fallback_host
        .database()
        .create_session("default", None, Some("Manual".into()))
        .unwrap()
        .id;
    for prompt in ["First fallback prompt", "Second fallback prompt"] {
        let turn = fallback_host
            .submit(&fallback_session, prompt)
            .unwrap()
            .join()
            .unwrap();
        assert!(matches!(turn.result.update, MessageUpdate::Append { .. }));
    }
    assert_eq!(
        fallback_server.model_requests().len(),
        1,
        "failed /models probe must be negative-cached on the ChatHost"
    );
    fallback_host.close().unwrap();
}

#[test]
fn user_workflow_reusing_compact_receives_discovered_context_window_in_context() {
    let server = Server::start_with_models(
        vec![
            usage_reply("Turn one answer", 40),
            usage_reply("Turn two answer", 90),
            Reply::text("## Goal\nWorkflow reused discovered window"),
        ],
        vec![Reply::models(json!([
            {"id": "fixture-model", "max_model_len": 100}
        ]))],
    );
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-11-13",
        "dependencies": {
            "core": { "package": "mfn-core", "version": format!("={}", env!("CARGO_PKG_VERSION")) },
            "code": { "package": "mfn-code", "version": format!("={}", env!("CARGO_PKG_VERSION")) },
            "llm": { "package": "mfn-llm", "version": format!("={}", env!("CARGO_PKG_VERSION")) }
        },
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
                        "context": {
                            "type": "object",
                            "default": {
                                "title": { "source": "provisional", "revision": 0, "attempted": false, "enabled": true },
                                "model_context": { "revision": 0 },
                                "compaction": { "enabled": true, "context_window": 128000, "reserve_tokens": 16384, "keep_recent_tokens": 20000, "custom_instructions": "" }
                            }
                        },
                        "model": { "type": "string", "default": "fixture-model", "secret": false },
                        "base_url": { "type": "string", "default": "https://api.openai.com/v1", "secret": false },
                        "api_key": { "type": "string", "default": "synthetic-key", "secret": true }
                    }
                }
            },
            {
                "id": "chat",
                "kind": "builtin.llm",
                "config": {
                    "model": { "$ref": "params.model" },
                    "base_url": { "$ref": "params.base_url" },
                    "api_key": { "$ref": "params.api_key" },
                    "capture_errors": true
                },
                "inputs": {
                    "messages": { "from": { "node": "params", "port": "messages" } }
                }
            },
            {
                "id": "turn",
                "kind": "builtin.code",
                "config": {
                    "language": "cel",
                    "inputs": {
                        "text": "string"
                    },
                    "code": {
                        "append_messages": "[{'role': 'assistant', 'content': text}]"
                    }
                },
                "inputs": {
                    "text": { "from": { "node": "chat", "port": "text" } }
                }
            },
            {
                "id": "compact",
                "kind": "workflow.call",
                "config": { "flow": "compact" },
                "inputs": {
                    "params.messages": { "from": { "node": "params", "port": "messages" } },
                    "params.append_messages": { "from": { "node": "turn", "port": "append_messages" } },
                    "params.usages": { "from": { "node": "chat", "port": "usages" } },
                    "params.context": { "from": { "node": "params", "port": "context" } },
                    "params.model": { "from": { "node": "params", "port": "model" } },
                    "params.base_url": { "from": { "node": "params", "port": "base_url" } },
                    "params.api_key": { "from": { "node": "params", "port": "api_key" } }
                }
            },
            {
                "id": "route",
                "kind": "builtin.if_else",
                "config": {
                    "branches": [{
                        "id": "replaced",
                        "condition": {
                            "source": { "output": "compact.compacted", "path": "" },
                            "operator": "eq",
                            "value": true
                        }
                    }]
                }
            },
            {
                "id": "append_result",
                "kind": "builtin.code",
                "config": {
                    "language": "cel",
                    "inputs": {
                        "append_messages": { "list": "object" },
                        "turn_usages": { "list": "object" },
                        "compact_usages": { "list": "object" }
                    },
                    "code": {
                        "value": "{'status': 'succeeded', 'messages': append_messages, 'usages': turn_usages + compact_usages}"
                    }
                },
                "inputs": {
                    "append_messages": { "from": { "node": "turn", "port": "append_messages" } },
                    "turn_usages": { "from": { "node": "chat", "port": "usages" } },
                    "compact_usages": { "from": { "node": "compact", "port": "usages" } }
                }
            },
            {
                "id": "replace_result",
                "kind": "builtin.code",
                "config": {
                    "language": "cel",
                    "inputs": {
                        "context_messages": { "list": "object" },
                        "display_messages": { "list": "object" },
                        "turn_usages": { "list": "object" },
                        "compact_usages": { "list": "object" }
                    },
                    "code": {
                        "value": "{'status': 'succeeded', 'context_messages': context_messages, 'display_messages': display_messages, 'usages': turn_usages + compact_usages}"
                    }
                },
                "inputs": {
                    "context_messages": { "from": { "node": "compact", "port": "messages" } },
                    "display_messages": { "from": { "node": "turn", "port": "append_messages" } },
                    "turn_usages": { "from": { "node": "chat", "port": "usages" } },
                    "compact_usages": { "from": { "node": "compact", "port": "usages" } }
                }
            }
        ],
        "edges": [
            { "from_node": "compact", "to_node": "route" },
            { "from_node": "route", "from_output": "replaced", "to_node": "replace_result" },
            { "from_node": "route", "from_output": "else", "to_node": "append_result" }
        ],
        "outputs": [
            { "name": "append", "node": "append_result", "port": "value", "optional": true },
            { "name": "replace_context", "node": "replace_result", "port": "value", "optional": true }
        ]
    }))
    .unwrap();

    let profile = WorkflowProfile::new(
        "reuse-discovered",
        definition,
        WorkflowArguments::try_from(json!({
            "params.model": "fixture-model",
            "params.base_url": server.base_url,
            "params.api_key": "synthetic-key"
        }))
        .unwrap(),
    )
    .unwrap();
    let mut config = HostConfig::new(profile);
    config.automatic_titles = false;
    config.automatic_compaction = CompactionConfig {
        enabled: true,
        context_window: None,
        reserve_tokens: 20,
        keep_recent_tokens: 6,
        custom_instructions: String::new(),
    };
    let root = tempfile::tempdir().unwrap();
    let host = ChatHost::open(root.path().join("chat.sqlite"), config).unwrap();
    let session_id = host
        .database()
        .create_session("reuse-discovered", None, Some("Manual".into()))
        .unwrap()
        .id;

    let first = host
        .submit(&session_id, "First prompt to seed history")
        .unwrap()
        .join()
        .unwrap();
    assert!(matches!(first.result.update, MessageUpdate::Append { .. }));
    let second = host
        .submit(
            &session_id,
            "Second prompt triggering workflow-owned compaction",
        )
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(server.model_requests().len(), 1);
    assert_eq!(
        server.requests().len(),
        3,
        "workflow should run turn 1, turn 2, and mf-chat.chat-compact using the discovered context_window (100)"
    );
    assert!(matches!(
        second.result.update,
        MessageUpdate::ReplaceContext { .. }
    ));
    host.close().unwrap();
}

#[test]
fn cancelled_context_window_probe_is_not_negatively_cached() {
    let mut slow_models = Reply::models(json!([
        {"id": "fixture-model", "context_window": 100}
    ]));
    slow_models.header_delay = std::time::Duration::from_millis(600);
    let server = Server::start_with_models(
        vec![
            usage_reply("Turn one after cancel", 40),
            usage_reply("Turn two", 90),
            Reply::text("## Goal\nCompacted after retry"),
        ],
        vec![
            slow_models,
            Reply::models(json!([
                {"id": "fixture-model", "context_window": 100}
            ])),
        ],
    );
    let root = tempfile::tempdir().unwrap();
    let host = plain_chat_host(
        root.path(),
        &server,
        false,
        CompactionConfig {
            enabled: true,
            context_window: None,
            reserve_tokens: 20,
            keep_recent_tokens: 4,
            custom_instructions: String::new(),
        },
    );
    let session_id = host
        .database()
        .create_session("default", None, Some("Manual".into()))
        .unwrap()
        .id;

    let cancelled_job = host
        .submit(&session_id, "Will cancel during probe")
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while server.model_requests().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "context_window /models probe did not arrive"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cancelled_job.cancel();
    let err = cancelled_job.join().unwrap_err();
    assert!(
        err.to_string().contains("cancelled") || err.to_string().contains("failed"),
        "{err}"
    );

    // Subsequent turns must re-probe /models rather than using a negative cache entry.
    let first = host
        .submit(&session_id, "First turn details to summarize")
        .unwrap()
        .join()
        .unwrap();
    assert!(matches!(first.result.update, MessageUpdate::Append { .. }));
    let second = host
        .submit(&session_id, "Retry turn")
        .unwrap()
        .join()
        .unwrap();
    assert!(
        matches!(second.result.update, MessageUpdate::ReplaceContext { .. }),
        "second={:?}, model_requests={}, chat_requests={}",
        second.result,
        server.model_requests().len(),
        server.requests().len(),
    );
    assert_eq!(server.model_requests().len(), 2);
    host.close().unwrap();
}
