use super::{
    compiled,
    example_support::{CODING_AGENT, streamed_tool_calls},
    next_completion,
};
use mf_chat::{ChatDatabase, ConversationResult, MessageUpdate, contract::CompletionState};
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

fn submit(
    binary: &Path,
    db: &Path,
    id: &str,
    mut inputs: Value,
    prompt: &str,
) -> ConversationResult {
    inputs["params.model"] = "example-model".into();
    inputs["params.api_key"] = "synthetic-key".into();
    let launch = ChatLaunch::prepare(
        binary,
        ChatOptions {
            database_path: db.to_owned(),
            profile: "coding".into(),
            run: RunOptions {
                inputs: Some(inputs.to_string()),
                ..Default::default()
            },
        },
    )
    .unwrap();
    let controller = ChatController::open(launch).unwrap();
    controller
        .send(ChatCommand::Submit {
            session_id: Some(id.into()),
            prompt: prompt.into(),
        })
        .unwrap();
    let resolution = next_completion(&controller);
    controller.close().unwrap();
    let CompletionState::Committed(completion) = resolution.state else {
        panic!("expected a committed coding turn")
    };
    completion.result
}

#[test]
fn reopened_coding_session_marks_only_workspace_transitions_before_their_user_turns() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("A");
    let b = root.path().join("B with spaces");
    let sequence = [&a, &a, &b, &b, &a];
    let server = Server::start(vec![Reply::stream_text("Done."); sequence.len()]);
    let binary = compiled(root.path(), &server, CODING_AGENT);
    let path = root.path().join("chat.sqlite");
    let id = ChatDatabase::open(&path)
        .unwrap()
        .create_session(
            "coding",
            Some("Keep the session policy.".into()),
            Some("Manual title".into()),
        )
        .unwrap()
        .id;
    for dir in [&a, &b] {
        fs::create_dir(dir).unwrap();
    }
    let mut expected = Vec::new();
    for (i, workspace) in sequence.into_iter().enumerate() {
        let prompt = format!("Inspect turn {i}.");
        let result = submit(
            &binary,
            &path,
            &id,
            json!({"params.workspace_root": workspace}),
            &prompt,
        );
        assert_eq!(result.final_text(), Some("Done."));
        assert_eq!(result.display_messages().len(), 1);
        let requests = server.requests();
        let request = requests.last().unwrap();
        let messages = super::messages(request);
        assert!(
            messages
                .iter()
                .any(|m| m["content"] == "Keep the session policy.")
        );
        if i == 0 || workspace != sequence[i - 1] {
            let notice = if i == 0 {
                format!("Workspace root: {}", json!(workspace))
            } else {
                format!(
                    "Workspace changed from {} to {}. Reinspect files here before relying on earlier tool results.",
                    json!(sequence[i - 1]),
                    json!(workspace)
                )
            };
            expected.push((json!(notice), json!(prompt)));
        }
        let notices: Vec<_> = messages
            .windows(2)
            .filter(|pair| {
                pair[0]["role"] == "system"
                    && pair[0]["content"]
                        .as_str()
                        .unwrap()
                        .starts_with("Workspace ")
            })
            .map(|pair| (pair[0]["content"].clone(), pair[1]["content"].clone()))
            .collect();
        assert_eq!(notices, expected);
        assert!(
            request.body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m.get("metadata").is_none())
        );
    }
    assert_eq!(server.requests().len(), sequence.len());
}

#[test]
fn coding_rules_follow_the_active_workspace_without_entering_the_saved_transcript() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("projects");
    let team = base.join("team");
    let a = team.join("A");
    let b = team.join("B");
    let outside = root.path().join("projects-other/worktree");
    let user = root.path().join("personal policy");
    for dir in [&a, &b, &user, &outside, &a.join("nested")] {
        fs::create_dir_all(dir).unwrap();
    }
    let rule = |dir: &Path, name: &str, content: &str| {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    };
    let personal = rule(&user, "AGENTS.md", "Personal rules");
    let shared = rule(&base, "AGENTS.md", "Shared rules");
    let group = rule(&team, "AGENTS.md", "Team rules");
    let original =
        "Use the project conventions.\n".repeat(2_100) + "Final rule on the last page.\n";
    let local_a = rule(&a, "AGENTS.override.md", &original);
    rule(&a, "AGENTS.md", "Shadowed project rules");
    rule(&a, ".gitignore", "AGENTS*\n");
    rule(
        &a.join("nested"),
        "AGENTS.md",
        "Nested rules do not apply at the root",
    );
    rule(root.path(), "AGENTS.md", "Outside the boundary");
    rule(
        outside.parent().unwrap(),
        "AGENTS.md",
        "Not a path-component prefix",
    );
    let local_b = rule(&b, "CLAUDE.md", "Rules for B");
    let server = Server::start(vec![Reply::stream_text("Done."); 7]);
    let binary = compiled(root.path(), &server, CODING_AGENT);
    let db = root.path().join("chat.sqlite");
    let id = ChatDatabase::open(&db)
        .unwrap()
        .create_session("coding", None, Some("Manual title".into()))
        .unwrap()
        .id;
    let policy = json!({"params.user_instruction_dir": user, "params.instruction_boundary": base});
    let check = |workspace: &Path, mut inputs: Value, expected: &[&Path]| {
        inputs["params.workspace_root"] = json!(workspace);
        assert_eq!(
            submit(&binary, &db, &id, inputs, "Check the current rules.").final_text(),
            Some("Done.")
        );
        let requests = server.requests();
        let messages = super::messages(requests.last().unwrap());
        let active = messages
            .iter()
            .rposition(|m| {
                m["role"] == "system" && m["content"].as_str().unwrap().starts_with("Workspace ")
            })
            .unwrap();
        let prompts: Vec<_> = expected.iter().map(|p| json!({"role":"system", "content":format!("Instructions from {}:\n{}", json!(p), fs::read_to_string(p).unwrap())})).collect();
        assert!(
            messages[active + 1..].starts_with(&prompts),
            "incorrect instruction order or content for {}",
            workspace.display()
        );
        assert_eq!(messages[active + 1 + prompts.len()]["role"], "user");
        assert_eq!(
            messages
                .iter()
                .filter(|m| m["role"] == "system"
                    && m["content"]
                        .as_str()
                        .unwrap()
                        .starts_with("Instructions from "))
                .count(),
            prompts.len()
        );
    };
    check(&a, json!({}), &[&local_a]);
    check(&a, policy.clone(), &[&personal, &shared, &group, &local_a]);
    check(&b, policy.clone(), &[&personal, &shared, &group, &local_b]);
    fs::write(&local_b, "Updated rules for B").unwrap();
    check(
        &b,
        json!({"params.user_instruction_dir": b, "params.instruction_boundary": base}),
        &[&shared, &group, &local_b],
    );
    rule(&b, "AGENTS.override.md", " \n");
    check(&b, json!({}), &[]);
    check(&outside, policy, &[&personal]);
    let unsafe_rule = outside.join("AGENTS.md");
    std::os::unix::fs::symlink(&local_a, &unsafe_rule).unwrap();
    check(
        &outside,
        json!({"params.load_instructions":false, "params.user_instruction_dir":root.path().join("absent")}),
        &[],
    );
    let error = std::process::Command::new(&binary).args(["--inputs", &json!({
        "params.workspace_root": outside, "params.messages":[{"role":"user","content":"Reject unsafe rules"}],
        "params.model":"example-model", "params.api_key":"synthetic-key"
    }).to_string()]).output().unwrap();
    assert!(!error.status.success());
    let stderr = String::from_utf8_lossy(&error.stderr);
    assert!(
        stderr.contains("regular file with exactly one hard link"),
        "{stderr}"
    );
    assert_eq!(server.requests().len(), 7);
    let database = ChatDatabase::open(&db).unwrap();
    let context = database.model_context(&id).unwrap().messages;
    let audit: Vec<_> = database
        .messages(&id, 32, None)
        .unwrap()
        .messages
        .into_iter()
        .map(|r| r.message.payload)
        .collect();
    assert_eq!(audit, context);
    assert_eq!(audit.len(), 14);
    assert!(
        audit
            .iter()
            .all(|m| !matches!(m, mfn_llm::ChatMessage::System { .. }))
    );
}

#[test]
fn command_output_reaches_the_terminal_before_exit_and_only_the_tail_reaches_the_model() {
    let root = tempfile::tempdir().unwrap();
    let work = root.path().join("working copy");
    fs::create_dir(&work).unwrap();
    let early = "First diagnostic from the subprocess";
    let last = "Last diagnostic from the subprocess";
    let noise = "uninteresting bytes\n".repeat(4000);
    fs::write(work.join("early"), format!("{early}\n")).unwrap();
    fs::write(work.join("noise"), &noise).unwrap();
    fs::write(work.join("diagnose.sh"), format!("cat early; while [ ! -f release ]; do sleep 0.05; done; cat noise; printf '%s\\n' '{last}'; exit 7")).unwrap();
    let server = Server::start(vec![
        streamed_tool_calls(json!([call(
            "diagnostic",
            "bash",
            json!({"command":"/bin/sh diagnose.sh", "timeout":20})
        )])),
        Reply::stream_text("Command inspected."),
        Reply::text("Diagnostics"),
    ]);
    let binary = compiled(root.path(), &server, CODING_AGENT);
    let path = root.path().join("chat.sqlite");
    let mut terminal = super::ChatTerminal::with_args(
        &binary,
        &path,
        &["--params.workspace_root", work.to_str().unwrap()],
    );
    terminal.wait_for(&["What would you like"]);
    terminal.send(b"Run diagnostics\x1b\r");
    terminal.wait_for(&[
        "Command output (latest)",
        early,
        "Tool call bash [diagnostic]",
    ]);
    assert_eq!(server.requests().len(), 1);
    fs::write(work.join("release"), "").unwrap();
    terminal.wait_for(&["Command inspected.", "Saved"]);
    terminal.quit();
    let requests = server.requests();
    let result = tool_result(&requests[1], "diagnostic");
    assert_eq!(result["exit_code"], 7);
    assert_eq!(result["stdout_truncated"], true);
    assert!(
        result["stdout"]
            .as_str()
            .unwrap()
            .ends_with(&format!("{last}\n"))
    );
    assert!(!requests[1].body.to_string().contains(early));
    let log = result["stdout_path"].as_str().unwrap();
    assert_eq!(
        fs::read_to_string(log).unwrap(),
        format!("{early}\n{noise}{last}\n")
    );
    fs::remove_file(log).unwrap();
    let db = ChatDatabase::open(path).unwrap();
    let id = &db.list_sessions(false, 10, None).unwrap().sessions[0].id;
    let page = db.display_page(id, 10, None).unwrap();
    assert_eq!(page.messages.len(), 4);
    assert!(page.messages.iter().all(|m| m.unfinished.is_empty()));
    assert!(
        !serde_json::to_string(&db.model_context(id).unwrap())
            .unwrap()
            .contains(early)
    );
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
    assert!(
        super::messages(&requests[0])
            .iter()
            .any(|m| m["role"] == "system"
                && m["content"] == format!("Workspace root: {}", json!(worktree)))
    );
    for request in &requests {
        let body = request.body.to_string();
        assert!(
            !request.body["tools"]
                .to_string()
                .contains(worktree.to_str().unwrap())
        );
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
