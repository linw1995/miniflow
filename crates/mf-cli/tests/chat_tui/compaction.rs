use super::{CODING_AGENT, call, compiled, submit};
use crate::{example_support::streamed_tool_calls_with_usage, gated};
use mf_chat::{ChatDatabase, MessageUpdate, contract::CompletionState};
use mf_tui::{
    chat::{ChatCommand, ChatController, ChatEvent, ChatLaunch, ChatOptions},
    run::RunOptions,
};
use mfn_llm::test_support::{Reply, Server};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const RULE: &str = "Current workspace instructions only.";
const SUMMARY: &str = "The first file was checked and the command already ran.";
const CORRECTION: &str = "Report only the differences.";

struct Fixture {
    root: tempfile::TempDir,
    work: PathBuf,
    db: PathBuf,
    id: String,
    server: Server,
    release_batch: Arc<AtomicBool>,
    binary: PathBuf,
}

impl Fixture {
    fn new(replies: impl IntoIterator<Item = Reply>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        fs::create_dir(&work).unwrap();
        for (name, marker) in [("first", "old"), ("second", "new")] {
            fs::write(
                work.join(name),
                format!("{marker}{}\n", "x".repeat(73)).repeat(610),
            )
            .unwrap();
        }
        fs::write(work.join("AGENTS.md"), RULE).unwrap();
        let (last_batch, release_batch) = gated(
            streamed_tool_calls_with_usage(
                json!([call("read-2", "read", json!({"path":"second"}))]),
                15750,
            ),
            String::new(),
        );
        let mut sequence = vec![
            streamed_tool_calls_with_usage(
                json!([
                    call("once", "bash", json!({"command":"echo run >> executions"})),
                    call("read-1", "read", json!({"path":"first"}))
                ]),
                4200,
            ),
            last_batch,
        ];
        sequence.extend(replies);
        let server = Server::start_with_models(
            sequence,
            vec![Reply::models(json!([{"id":"example-model","context_window":32768}])); 2],
        );
        let binary = compiled(root.path(), &server, CODING_AGENT);
        let db = root.path().join("chat.sqlite");
        let id = ChatDatabase::open(&db)
            .unwrap()
            .create_session("coding", None, Some("Manual title".into()))
            .unwrap()
            .id;
        Self {
            root,
            work,
            db,
            id,
            server,
            release_batch,
            binary,
        }
    }

    fn start_with_steering(&self) -> ChatController {
        let inputs = json!({"params.workspace_root":self.work, "params.model":"example-model", "params.api_key":"synthetic-key"});
        let launch = ChatLaunch::prepare(
            &self.binary,
            ChatOptions {
                database_path: self.db.clone(),
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
                session_id: Some(self.id.clone()),
                prompt: "Inspect both files.".into(),
            })
            .unwrap();
        self.server.wait_for_requests(2);
        let invocation = std::iter::from_fn(|| controller.try_event())
            .find_map(|event| match event {
                ChatEvent::Submitted { invocation, .. } => Some(invocation),
                _ => None,
            })
            .expect("admission precedes the model request");
        controller.steer(&invocation, CORRECTION.into()).unwrap();
        self.release_batch.store(true, Ordering::Release);
        controller
    }

    fn check_no_replay(&self, requests: usize) {
        assert_eq!(self.server.requests().len(), requests);
        assert_eq!(
            fs::read_to_string(self.work.join("executions")).unwrap(),
            "run\n"
        );
    }
}

fn tool_ids(messages: &[Value]) -> (Vec<&str>, Vec<&str>) {
    (
        messages
            .iter()
            .flat_map(|m| m["tool_calls"].as_array().into_iter().flatten())
            .map(|call| call["id"].as_str().unwrap())
            .collect(),
        messages
            .iter()
            .filter_map(|m| m["tool_call_id"].as_str())
            .collect(),
    )
}

fn check_audit(database: &ChatDatabase, id: &str, work: &Path, length: usize, ids: &[&str]) {
    let audit = database.messages(id, 32, None).unwrap().messages;
    assert_eq!(audit.len(), length);
    let messages: Vec<_> = audit.iter().map(|r| json!(r.message.payload)).collect();
    let (calls, results) = tool_ids(&messages);
    assert_eq!(calls, ids);
    assert_eq!(results, ids);
    for (id, file) in [("read-1", "first"), ("read-2", "second")] {
        let message = messages.iter().find(|m| m["tool_call_id"] == id).unwrap();
        let result: Value = serde_json::from_str(message["content"].as_str().unwrap()).unwrap();
        assert_eq!(
            result["content"],
            fs::read_to_string(work.join(file)).unwrap()
        );
    }
    assert!(!json!(messages).to_string().contains(SUMMARY));
    assert!(!json!(messages).to_string().contains(RULE));
}

#[test]
fn checkpoint_precedes_the_next_model_request_without_replaying_or_replacing_tool_audit() {
    let f = Fixture::new([
        Reply::text(SUMMARY),
        streamed_tool_calls_with_usage(
            json!([call("list", "bash", json!({"command":"ls -1"}))]),
            13000,
        ),
        Reply::stream_text("Checked both files."),
        Reply::stream_text("Reopened."),
    ]);
    crate::fail_save(&f.db, true);
    let controller = f.start_with_steering();
    let CompletionState::Uncommitted(pending) = crate::next_completion(&controller).state else {
        panic!("expected a save-only candidate")
    };
    f.check_no_replay(5);
    crate::fail_save(&f.db, false);
    controller.send(ChatCommand::Save).unwrap();
    let CompletionState::Committed(completion) = crate::next_completion(&controller).state else {
        panic!("expected saved checkpoint")
    };
    assert_eq!(completion.result, pending.completion.result);
    controller.close().unwrap();
    let result = completion.result;
    assert_eq!(result.final_text(), Some("Checked both files."));
    assert_eq!(
        result
            .usages
            .iter()
            .map(|u| u.attribution.as_ref().unwrap().purpose.as_deref().unwrap())
            .collect::<Vec<_>>(),
        [
            "conversation",
            "conversation",
            "compaction",
            "conversation",
            "conversation"
        ]
    );
    let MessageUpdate::ReplaceContext {
        context_messages,
        display_messages,
    } = &result.update
    else {
        panic!("expected checkpoint")
    };
    let steering = json!({"role":"user", "content":CORRECTION});
    assert_eq!(display_messages.len(), 9);
    assert_eq!(json!(display_messages[5]), steering);
    let requests = f.server.requests();
    assert!(requests[2].body.get("tools").is_none());
    let summary_request = requests[2].body["messages"].to_string();
    assert!(summary_request.contains("Inspect both files.") && summary_request.contains("oldxxxx"));
    assert!(!summary_request.contains(RULE) && !summary_request.contains("newxxxx"));
    assert!(!summary_request.contains(CORRECTION));
    let third = requests[3].body["messages"].as_array().unwrap();
    assert_eq!(third.last(), Some(&steering));
    let (calls, results) = tool_ids(third);
    assert_eq!(calls, ["read-2"]);
    assert_eq!(calls, results);
    let wire = json!(third).to_string();
    assert!(wire.contains(SUMMARY) && wire.contains(RULE) && wire.contains("newxxxx"));
    assert!(!wire.contains("oldxxxx"));
    let fourth = requests[4].body["messages"].as_array().unwrap();
    assert!(fourth.starts_with(third));
    let (calls, results) = tool_ids(fourth);
    assert_eq!(calls, ["read-2", "list"]);
    assert_eq!(results, calls);
    {
        let db = ChatDatabase::open(&f.db).unwrap();
        check_audit(
            &db,
            &f.id,
            &f.work,
            10,
            &["once", "read-1", "read-2", "list"],
        );
        assert_eq!(db.model_context(&f.id).unwrap().messages, *context_messages);
        let saved = json!(context_messages).to_string();
        assert!(
            saved.contains(SUMMARY) && !saved.contains(RULE) && !saved.contains("Workspace root:")
        );
    }
    f.check_no_replay(5);
    let other = f.root.path().join("other");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("AGENTS.md"), "New workspace rules.").unwrap();
    assert_eq!(
        submit(
            &f.binary,
            &f.db,
            &f.id,
            json!({"params.workspace_root":other}),
            "Continue."
        )
        .final_text(),
        Some("Reopened.")
    );
    let resumed = &f.server.requests()[5].body["messages"];
    assert!(
        resumed.to_string().contains(SUMMARY)
            && resumed.to_string().contains("New workspace rules.")
    );
    assert!(!resumed.to_string().contains(RULE));
    assert_eq!(
        resumed
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| **m == steering)
            .count(),
        1
    );
    assert_eq!(tool_ids(resumed.as_array().unwrap()).1, ["read-2", "list"]);
    f.check_no_replay(6);
}

#[test]
fn failed_mid_turn_summary_keeps_completed_effects_and_usage_without_a_continuation() {
    let mut incomplete = Reply::text("Partial summary must not be used.");
    incomplete.body = incomplete.body.replace("\"stop\"", "\"length\"");
    let f = Fixture::new([incomplete]);
    let controller = f.start_with_steering();
    let mut drafts = Vec::new();
    let resolution = crate::observe_completion(&controller, |event| {
        if let ChatEvent::UnsentSteering { prompts, .. } = event {
            drafts.extend_from_slice(prompts);
        }
    });
    controller.close().unwrap();
    assert_eq!(drafts, [CORRECTION]);
    let CompletionState::Committed(completion) = resolution.state else {
        panic!("expected a recorded compaction failure")
    };
    let result = completion.result;
    assert!(matches!(result.update, MessageUpdate::Append { .. }));
    assert_eq!(result.status, "failed");
    assert!(
        result
            .error
            .as_deref()
            .unwrap()
            .contains("summary is incomplete")
    );
    assert_eq!(result.usages.len(), 3);
    assert_eq!(
        result.usages[2]
            .attribution
            .as_ref()
            .unwrap()
            .purpose
            .as_deref(),
        Some("compaction")
    );
    let db = ChatDatabase::open(&f.db).unwrap();
    assert_eq!(db.model_context(&f.id).unwrap().revision, 0);
    check_audit(&db, &f.id, &f.work, 6, &["once", "read-1", "read-2"]);
    f.check_no_replay(3);
}

#[test]
fn cancelling_a_pending_summary_does_not_resume_or_publish_a_tool_exchange() {
    let (summary, release) = gated(Reply::text(SUMMARY), String::new());
    let f = Fixture::new([summary]);
    let controller = f.start_with_steering();
    f.server.wait_for_requests(3);
    assert!(f.server.requests()[2].body.get("tools").is_none());
    controller.cancel();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(
            Instant::now() < deadline,
            "pending summary was not cancelled"
        );
        if let Some(ChatEvent::Finished { resolution, .. }) = controller.try_event() {
            let CompletionState::Finished(turn) = resolution.unwrap().state else {
                panic!("no terminal result is eligible for save")
            };
            assert_eq!(turn.state, "cancelled");
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    release.store(true, Ordering::Release);
    controller.close().unwrap();
    let db = ChatDatabase::open(&f.db).unwrap();
    assert_eq!(db.messages(&f.id, 32, None).unwrap().messages.len(), 1);
    assert_eq!(db.model_context(&f.id).unwrap().revision, 0);
    f.check_no_replay(3);
}
