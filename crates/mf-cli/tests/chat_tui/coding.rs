use super::{
    compiled,
    example_support::{CODING_AGENT, streamed_tool_calls},
    next_completion,
};
use mf_chat::{ChatDatabase, MessageUpdate, contract::CompletionState};
use mf_tui::{
    chat::{ChatCommand, ChatController, ChatLaunch, ChatOptions},
    run::RunOptions,
};
use mfn_llm::test_support::{Reply, Request, Server};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path};

fn call(id: &str, name: &str, arguments: Value) -> Value {
    json!({"id":id, "type":"function", "function":{"name":name, "arguments":arguments.to_string()}})
}

fn tool_result(request: &Request, id: &str) -> Value {
    let messages = request.body["messages"].as_array().unwrap();
    let message = messages
        .iter()
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
        .unwrap_or_else(|| panic!("missing tool result {id}"));
    serde_json::from_str(message["content"].as_str().unwrap()).unwrap()
}

#[test]
fn coding_agent_repairs_a_failing_workspace_and_reopens_its_tool_transcript() {
    const OLD: &str = "printf 'Hello, world!\\n'";
    const NEW: &str = r#"printf 'Hello, %s!\n' "${1:-world}""#;
    const NOTE: &str = "Run `/bin/sh greet.sh Ada` to greet Ada.\n";
    const ANSWER: &str = "Fixed the greeting, verified both checks, and added docs/USAGE.md.";
    let root = tempfile::tempdir().unwrap();
    let server = Server::start(vec![
        streamed_tool_calls(json!([
            call(
                "grep",
                "grep",
                json!({"pattern":"Hello, world!", "literal":true})
            ),
            call("scripts", "find", json!({"pattern":"*.sh", "kind":"file"})),
            call("listing", "ls", json!({})),
            call("read", "read", json!({"path":"greet.sh"})),
            call(
                "override",
                "bash",
                json!({"workspace_root":root.path(),"command":"printf unexpected > stray"})
            ),
            call("before", "bash", json!({"command":"/bin/sh ./test.sh"})),
        ])),
        streamed_tool_calls(json!([
            call(
                "fix",
                "edit",
                json!({"path":"greet.sh", "edits":[{"old_text":OLD, "new_text":NEW}]})
            ),
            call(
                "after",
                "bash",
                json!({"command":"/bin/sh -n ./greet.sh && /bin/sh ./test.sh", "timeout":10})
            ),
            call("docs", "mkdir", json!({"path":"docs"})),
            call(
                "note",
                "write",
                json!({"path":"docs/USAGE.md", "content":NOTE})
            ),
            call(
                "review",
                "bash",
                json!({"command":"git diff --check && git diff -- greet.sh"})
            ),
        ])),
        Reply::stream_text(ANSWER),
    ]);
    let binary = compiled(root.path(), &server, CODING_AGENT);
    let worktree = root.path().join("working copy");
    fs::create_dir(&worktree).unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/agent-coding/workspace");
    for file in ["greet.sh", "test.sh"] {
        fs::copy(fixture.join(file), worktree.join(file)).unwrap();
    }
    for args in [&["init", "-q"][..], &["add", "--", "greet.sh", "test.sh"]] {
        assert!(
            std::process::Command::new("git")
                .current_dir(&worktree)
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let original = fs::read_to_string(worktree.join("greet.sh")).unwrap();

    let path = root.path().join("chat.sqlite");
    let id = ChatDatabase::open(&path)
        .unwrap()
        .create_session("coding", None, Some("Greeting repair".into()))
        .unwrap()
        .id;
    let inputs = json!({
        "params.model": "example-model",
        "params.api_key": "synthetic-coding-key",
        "params.workspace_root": worktree,
    });
    let options = ChatOptions {
        database_path: path.clone(),
        profile: "coding".into(),
        run: RunOptions {
            inputs: Some(inputs.to_string()),
            ..Default::default()
        },
    };
    let launch = ChatLaunch::prepare(&binary, options).unwrap();
    assert!(launch.contract.answer_stream && launch.contract.tool_call_delta_stream);
    let controller = ChatController::open(launch).unwrap();
    controller
        .send(ChatCommand::Submit {
            session_id: Some(id.clone()),
            prompt: "Fix greet.sh, run the tests and add usage instructions.".into(),
        })
        .unwrap();
    let CompletionState::Committed(completion) = next_completion(&controller).state else {
        panic!("expected a committed coding turn")
    };
    controller.close().unwrap();
    assert_eq!(
        completion.result.status, "succeeded",
        "{:?}",
        completion.result.error
    );
    assert_eq!(completion.result.final_text(), Some(ANSWER));
    assert!(matches!(
        completion.result.update,
        MessageUpdate::Append { .. }
    ));
    assert_eq!(completion.result.usages.len(), 3);

    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].body["messages"][0]["role"], "system");
    let tools = requests[0].body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 8);
    for (name, keys) in [
        ("mkdir", &["path"][..]),
        ("ls", &["path", "limit"][..]),
        (
            "find",
            &["pattern", "path", "kind", "max_depth", "limit"][..],
        ),
        (
            "grep",
            &["path", "pattern", "literal", "ignore_case", "limit"][..],
        ),
        ("read", &["path", "start_line", "limit"][..]),
        ("edit", &["path", "edits"]),
        ("write", &["path", "content"]),
        ("bash", &["command", "timeout"]),
    ] {
        let tool = tools
            .iter()
            .find(|t| t["function"]["name"] == name)
            .unwrap();
        let schema = &tool["function"]["parameters"];
        assert_eq!(schema["additionalProperties"], false);
        let public: BTreeSet<_> = schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(public, keys.iter().copied().collect(), "{name}");
        if name == "bash" {
            assert_eq!(schema["required"], json!(["command"]));
            assert_eq!(schema["properties"]["command"], json!({"type":"string"}));
        }
    }
    for request in &requests {
        let body = request.body.to_string();
        assert!(!body.contains(worktree.to_str().unwrap()));
        assert!(!body.contains("synthetic-coding-key"));
    }
    assert_eq!(
        tool_result(&requests[1], "scripts")["entries"],
        json!([{"path":"greet.sh", "kind":"file"}, {"path":"test.sh", "kind":"file"}])
    );
    assert_eq!(
        tool_result(&requests[1], "listing"),
        tool_result(&requests[1], "scripts")
    );
    let hits = tool_result(&requests[1], "grep");
    assert_eq!(hits["truncated"], false);
    assert!(
        hits["matches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|hit| hit["path"] == "greet.sh" && hit["text"] == OLD)
    );
    assert_eq!(tool_result(&requests[1], "read")["content"], original);
    assert_eq!(
        tool_result(&requests[1], "override")["error"],
        "unknown, secret, or configured argument"
    );
    let review = tool_result(&requests[2], "review");
    assert_eq!(review["exit_code"], 0);
    assert!(review["stdout"].as_str().unwrap().contains(NEW));
    let before = tool_result(&requests[1], "before");
    assert_eq!(before["exit_code"], 1);
    assert!(before["stderr"].as_str().unwrap().contains("Hello, Ada!"));
    assert_eq!(tool_result(&requests[2], "fix")["replacements"], 1);
    let after = tool_result(&requests[2], "after");
    assert_eq!(after["exit_code"], 0);
    assert_eq!(after["stdout"], "2 checks passed\n");
    assert_eq!(tool_result(&requests[2], "docs")["created"], true);
    assert_eq!(
        tool_result(&requests[2], "note")["bytes_written"],
        NOTE.len()
    );
    assert_eq!(
        fs::read_to_string(worktree.join("greet.sh")).unwrap(),
        original.replace(OLD, NEW)
    );
    assert_eq!(
        fs::read_to_string(worktree.join("docs/USAGE.md")).unwrap(),
        NOTE
    );
    assert!(!worktree.join("stray").exists());
    assert!(!root.path().join("stray").exists());

    let database = ChatDatabase::open(path).unwrap();
    let saved: Vec<_> = database
        .messages(&id, 50, None)
        .unwrap()
        .messages
        .into_iter()
        .map(|record| record.message.payload)
        .collect();
    assert!(
        saved
            .iter()
            .all(|message| !matches!(message, mfn_llm::ChatMessage::System { .. }))
    );
    assert!(matches!(saved[0], mfn_llm::ChatMessage::User { .. }));
    assert_eq!(&saved[1..], completion.result.display_messages());
    assert_eq!(database.model_context(&id).unwrap().messages, saved);
}
