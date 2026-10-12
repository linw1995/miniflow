use diesel::{Connection, connection::SimpleConnection};
use mf_chat::{ChatDatabase, ChatError, ChatHost, HostConfig, ModelProfile};
use mfn_llm::test_support::{Reply, Server, messages};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::Duration,
};

fn probe(server: &Server, scenario: &str, second_url: Option<&str>) -> Value {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "execution_probe", "--ignored", "--nocapture"])
        .env("MF_CHAT_TEST_SCENARIO", scenario)
        .env("MF_CHAT_TEST_URL", &server.base_url)
        .env("MF_CHAT_TEST_KEY", "synthetic-key")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
    if let Some(url) = second_url {
        command.env("MF_CHAT_TEST_SECOND_URL", url);
    }
    let mut child = command.spawn().unwrap();
    if matches!(
        scenario,
        "cancel" | "capacity" | "parallel" | "shutdown" | "title_rename" | "title_cancel"
    ) {
        server.wait_for_requests(
            if scenario == "title_rename" || scenario == "title_cancel" {
                2
            } else {
                1
            },
        );
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"continue\n")
            .unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    serde_json::from_str(
        text.lines()
            .find_map(|line| line.strip_prefix("CHAT_RESULT:"))
            .unwrap(),
    )
    .unwrap()
}

fn profile(name: &str, url: &str) -> ModelProfile {
    ModelProfile::new(name, json!({"model": name, "base_url": url, "api_key_env": "MF_CHAT_TEST_KEY", "timeout_ms": 3000})).unwrap()
}

#[test]
fn reopened_host_resumes_never_started_title_work() {
    let server = Server::start(vec![Reply::text("answer"), Reply::text("Resumed title")]);
    assert_eq!(
        probe(&server, "title_resume", None)["title"]["outcome"],
        "applied"
    );
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn title_cancellation_does_not_cancel_the_committed_conversation() {
    let mut title = Reply::text("late");
    title.header_delay = Duration::from_millis(1200);
    let server = Server::start(vec![Reply::text("answer"), title]);
    let result = probe(&server, "title_cancel", None);
    assert_eq!(result["title_state"], "cancelled");
    assert_eq!(result["turn_state"], "succeeded");
}

#[test]
fn automatic_title_uses_its_own_model_and_does_not_append_messages() {
    let conversation = Server::start(vec![Reply::text("answer")]);
    let summary = Server::start(vec![Reply::text("Generated title")]);
    let result = probe(&conversation, "title_auto", Some(&summary.base_url));
    assert_eq!(result["title"]["outcome"], "applied");
    assert_eq!(result["profile"], "second");
    assert_eq!(result["messages"], 2);
    assert_eq!(conversation.requests().len(), 1);
    let requests = summary.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["model"], "second");
    assert!(
        messages(&requests[0])[1]["content"]
            .as_str()
            .unwrap()
            .contains("assistant: answer")
    );
}

#[test]
fn manual_title_before_execution_skips_the_model_call() {
    let server = Server::start(vec![Reply::text("answer")]);
    let result = probe(&server, "title_before", None);
    assert_eq!(result["title"]["outcome"], "skipped");
    assert_eq!(result["session"]["title"], "Manual");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn manual_title_during_generation_preserves_model_usage() {
    let mut summary = Reply::text("Generated");
    summary.header_delay = Duration::from_millis(1200);
    let server = Server::start(vec![Reply::text("answer"), summary]);
    let result = probe(&server, "title_rename", None);
    assert_eq!(result["title"]["outcome"], "stale");
    assert_eq!(result["session"]["title"], "Manual");
    assert_eq!(result["usage"], 1);
    assert_eq!(result["messages"], 2);
}

#[test]
fn failed_title_preserves_conversation_and_explicit_retry_uses_a_new_call() {
    let mut failure = Reply::json(json!({"error":{"message":"synthetic title failure"}}));
    failure.status = 500;
    let server = Server::start(vec![
        Reply::text("answer"),
        failure,
        Reply::text("Recovered title"),
    ]);
    let result = probe(&server, "title_failure", None);
    assert_eq!(result["title"]["outcome"], "applied");
    assert_eq!(result["session"]["conversation_revision"], 1);
    assert_eq!(result["messages"], 2);
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn uncommitted_title_can_retry_storage_without_another_model_call() {
    let server = Server::start(vec![Reply::text("answer"), Reply::text("Generated title")]);
    let result = probe(&server, "title_persistence", None);
    assert_eq!(result["title"]["outcome"], "applied");
    assert_eq!(server.requests().len(), 2);
}
fn synchronize() {
    let mut byte = [0];
    std::io::stdin().read_exact(&mut byte).unwrap();
}

#[test]
#[ignore = "invoked as an isolated credential subprocess"]
fn execution_probe() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    let scenario = std::env::var("MF_CHAT_TEST_SCENARIO").unwrap();
    let url = std::env::var("MF_CHAT_TEST_URL").unwrap();
    let mut config = HostConfig::new(profile("default", &url));
    config.automatic_titles = false;
    if scenario == "title_auto" {
        config.automatic_titles = true;
        config.title_profile = Some("second".into());
    }
    if scenario == "capacity" {
        config.max_active = 1;
    }
    if scenario == "parallel" || scenario == "title_auto" {
        config.profiles.insert(
            "second".into(),
            profile("second", &std::env::var("MF_CHAT_TEST_SECOND_URL").unwrap()),
        );
    }
    let host = ChatHost::open(&path, config).unwrap();
    let system = if scenario == "no_metadata" {
        None
    } else {
        Some("Answer briefly".into())
    };
    let session = host
        .database()
        .create_session("default", system, None)
        .unwrap();
    let output = match scenario.as_str() {
        "title_resume" => {
            host.submit(&session.id, "first").unwrap().join().unwrap();
            let call_id = host.database().pending_titles(10).unwrap()[0].id.clone();
            host.close().unwrap();
            let reopened =
                ChatHost::open(&path, HostConfig::new(profile("default", &url))).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(result) = reopened.poll_titles().pop() {
                    let result = result.unwrap();
                    assert_eq!(result.invocation.call_id, call_id);
                    println!("CHAT_RESULT:{}", json!({"title":result}));
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
            reopened.close().unwrap();
            return;
        }
        "title_cancel" => {
            let completion = host.submit(&session.id, "first").unwrap().join().unwrap();
            let call = host.database().pending_titles(10).unwrap().pop().unwrap();
            let job = host.start_title(&call.id).unwrap();
            synchronize();
            job.cancel();
            assert!(job.join().is_err());
            json!({"title_state":host.database().call(&call.id).unwrap().state,
                "turn_state":host.database().turn(&completion.invocation.turn_id).unwrap().state})
        }
        "title_auto" => {
            let completion = host.submit(&session.id, "first").unwrap().join().unwrap();
            assert_eq!(
                host.database()
                    .turn(&completion.invocation.turn_id)
                    .unwrap()
                    .state,
                "succeeded"
            );
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let title = loop {
                let mut results = host.poll_titles();
                if let Some(result) = results.pop() {
                    break result.unwrap();
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            };
            json!({"title":title, "profile":host.database().call(&title.invocation.call_id).unwrap().profile,
                "messages":host.database().messages(&session.id, 10, None).unwrap().messages.len()})
        }
        "title_before" | "title_rename" | "title_failure" | "title_persistence" => {
            host.submit(&session.id, "first").unwrap().join().unwrap();
            let call = host.database().pending_titles(10).unwrap().pop().unwrap();
            if scenario == "title_before" {
                host.database()
                    .rename_session(&session.id, "Manual")
                    .unwrap();
            }
            let mut connection =
                diesel::SqliteConnection::establish(path.to_str().unwrap()).unwrap();
            if scenario == "title_persistence" {
                connection.batch_execute("CREATE TRIGGER reject_title BEFORE UPDATE ON model_calls WHEN NEW.purpose = 'title' AND NEW.state = 'succeeded' BEGIN SELECT RAISE(ABORT, 'injected title write failure'); END;").unwrap();
            }
            let job = host.start_title(&call.id).unwrap();
            if scenario == "title_rename" {
                synchronize();
                host.database()
                    .rename_session(&session.id, "Manual")
                    .unwrap();
            }
            let title = if scenario == "title_failure" {
                assert!(job.join().is_err());
                assert_eq!(
                    host.database()
                        .session(&session.id)
                        .unwrap()
                        .conversation_revision,
                    1
                );
                host.retry_title(&session.id).unwrap().join().unwrap()
            } else if scenario == "title_persistence" {
                let error = job.join().unwrap_err();
                let ChatError::Uncommitted { pending, .. } = error else {
                    panic!("{error:?}");
                };
                assert_eq!(pending.purpose, mf_chat::CallPurpose::Title);
                connection
                    .batch_execute("DROP TRIGGER reject_title;")
                    .unwrap();
                host.retry_title_completion(*pending).unwrap()
            } else {
                job.join().unwrap()
            };
            json!({"title":title, "session":host.database().session(&session.id).unwrap(),
                "messages":host.database().messages(&session.id, 10, None).unwrap().messages.len(),
                "usage":host.database().call(&title.invocation.call_id).unwrap().output_tokens})
        }
        "conversation" | "no_metadata" => {
            let first = host.submit(&session.id, "first").unwrap().join().unwrap();
            let second = host.submit(&session.id, "second").unwrap().join().unwrap();
            json!({"first":first, "second":second, "messages":host.database().history(&session.id, 1024).unwrap().history.len()})
        }
        "cancel" => {
            let job = host.submit(&session.id, "cancel me").unwrap();
            let invocation = job.invocation().clone();
            synchronize();
            job.cancel();
            assert!(job.join().is_err());
            json!({"state":host.database().turn(&invocation.turn_id).unwrap().state,
                "history":host.database().history(&session.id, 1024).unwrap().history.len()})
        }
        "capacity" => {
            let job = host.submit(&session.id, "pending").unwrap();
            synchronize();
            let other = host
                .database()
                .create_session("default", None, None)
                .unwrap();
            assert!(matches!(
                host.submit(&other.id, "blocked"),
                Err(ChatError::Capacity { .. })
            ));
            assert!(matches!(
                host.submit(&session.id, "duplicate"),
                Err(ChatError::SessionBusy { .. })
            ));
            job.cancel();
            assert!(job.join().is_err());
            json!({"other_messages":host.database().messages(&other.id, 10, None).unwrap().messages.len()})
        }
        "parallel" => {
            let first = host.submit(&session.id, "slow").unwrap();
            let invocation = first.invocation().clone();
            synchronize();
            let other = host
                .database()
                .create_session("second", None, None)
                .unwrap();
            let second = host.submit(&other.id, "fast").unwrap().join().unwrap();
            assert_eq!(
                host.database().turn(&invocation.turn_id).unwrap().state,
                "running"
            );
            first.cancel();
            assert!(first.join().is_err());
            json!({"fast":second.reply.text, "other_messages":host.database().history(&other.id, 1024).unwrap().history.len()})
        }
        "persistence" => {
            let mut connection =
                diesel::SqliteConnection::establish(path.to_str().unwrap()).unwrap();
            connection.batch_execute("CREATE TRIGGER reject_completion BEFORE UPDATE ON model_calls WHEN NEW.state = 'succeeded' BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;").unwrap();
            let error = host
                .submit(&session.id, "persist once")
                .unwrap()
                .join()
                .unwrap_err();
            let ChatError::Uncommitted { pending, .. } = error else {
                panic!("{error:?}");
            };
            let error = host.retry_completion(*pending).unwrap_err();
            let ChatError::Uncommitted { pending, .. } = error else {
                panic!("{error:?}");
            };
            assert_eq!(
                host.database()
                    .messages(&session.id, 10, None)
                    .unwrap()
                    .messages
                    .len(),
                1
            );
            connection
                .batch_execute("DROP TRIGGER reject_completion;")
                .unwrap();
            let completion = host.retry_completion(*pending).unwrap();
            json!({"text":completion.reply.text, "messages":host.database().messages(&session.id, 10, None).unwrap().messages.len()})
        }
        "cleanup" => {
            let mut connection =
                diesel::SqliteConnection::establish(path.to_str().unwrap()).unwrap();
            connection.batch_execute("CREATE TRIGGER reject_cleanup BEFORE UPDATE ON model_calls WHEN NEW.state = 'failed' BEGIN SELECT RAISE(ABORT, 'injected cleanup failure'); END;").unwrap();
            let error = host
                .submit(&session.id, "fail once")
                .unwrap()
                .join()
                .unwrap_err();
            let ChatError::Execution {
                invocation,
                cleanup,
                source,
            } = error
            else {
                panic!("{error:?}");
            };
            assert!(cleanup.is_some());
            assert!(source.to_string().contains("provider"));
            assert_eq!(
                host.database().turn(&invocation.turn_id).unwrap().state,
                "running"
            );
            connection
                .batch_execute("DROP TRIGGER reject_cleanup;")
                .unwrap();
            json!({"cleanup_failed":true})
        }
        "shutdown" => {
            let job = host.submit(&session.id, "pending").unwrap();
            let invocation = job.invocation().clone();
            synchronize();
            host.close().unwrap();
            assert!(job.join().is_err());
            let database = ChatDatabase::open(&path).unwrap();
            let state = database.turn(&invocation.turn_id).unwrap().state;
            println!("CHAT_RESULT:{}", json!({"state":state}));
            return;
        }
        _ => panic!("unknown scenario"),
    };
    host.close().unwrap();
    println!("CHAT_RESULT:{output}");
}

#[test]
fn embedded_conversation_preserves_two_turn_history_and_sparse_metadata() {
    let mut second = Reply::text("second answer");
    let mut body: Value = serde_json::from_str(&second.body).unwrap();
    body["usage"] = json!({"completion_tokens": 1});
    second.body = body.to_string();
    let server = Server::start(vec![Reply::text("first answer"), second]);
    let result = probe(&server, "conversation", None);
    assert_eq!(result["messages"], 4);
    assert_eq!(
        result["second"]["reply"]["usage"],
        json!({"output_tokens":1})
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        messages(&requests[0]),
        vec![
            json!({"role":"system","content":"Answer briefly"}),
            json!({"role":"user","content":"first"})
        ]
    );
    assert_eq!(
        messages(&requests[1]),
        vec![
            json!({"role":"system","content":"Answer briefly"}),
            json!({"role":"user","content":"first"}),
            json!({"role":"assistant","content":"first answer"}),
            json!({"role":"user","content":"second"})
        ]
    );
    assert_ne!(
        result["first"]["invocation"]["call_id"],
        result["second"]["invocation"]["call_id"]
    );
}

#[test]
fn cancellation_finalizes_the_turn_without_history() {
    let mut reply = Reply::text("late");
    reply.header_delay = Duration::from_millis(1200);
    let server = Server::start(vec![reply]);
    let result = probe(&server, "cancel", None);
    assert_eq!(result["state"], "cancelled");
    assert_eq!(result["history"], 0);
}

#[test]
fn absent_system_and_usage_do_not_skip_response_persistence() {
    let mut reply = Reply::text("answer");
    let mut body: Value = serde_json::from_str(&reply.body).unwrap();
    body.as_object_mut().unwrap().remove("usage");
    body["choices"][0]
        .as_object_mut()
        .unwrap()
        .remove("finish_reason");
    reply.body = body.to_string();
    let server = Server::start(vec![reply.clone(), reply]);
    let result = probe(&server, "no_metadata", None);
    assert_eq!(result["messages"], 4);
    assert!(result["first"]["reply"]["usage"].is_null());
    assert!(result["first"]["reply"]["finish_reason"].is_null());
    assert_eq!(messages(&server.requests()[0]).len(), 1);
}

#[test]
fn host_capacity_rejects_extra_work_without_creating_turns() {
    let mut reply = Reply::text("late");
    reply.header_delay = Duration::from_millis(1200);
    let server = Server::start(vec![reply]);
    assert_eq!(probe(&server, "capacity", None)["other_messages"], 0);
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn separate_sessions_progress_while_another_model_is_waiting() {
    let mut reply = Reply::text("slow");
    reply.header_delay = Duration::from_millis(2000);
    let slow = Server::start(vec![reply]);
    let fast = Server::start(vec![Reply::text("fast")]);
    let result = probe(&slow, "parallel", Some(&fast.base_url));
    assert_eq!(result["fast"], "fast");
    assert_eq!(result["other_messages"], 2);
}

#[test]
fn persistence_retry_does_not_repeat_the_model_request() {
    let server = Server::start(vec![Reply::text("saved answer")]);
    let result = probe(&server, "persistence", None);
    assert_eq!(result["text"], "saved answer");
    assert_eq!(result["messages"], 2);
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn failed_cleanup_preserves_original_error_and_is_recovered_on_close() {
    let mut reply = Reply::json(json!({"error":{"message":"synthetic failure"}}));
    reply.status = 500;
    let server = Server::start(vec![reply]);
    assert_eq!(probe(&server, "cleanup", None)["cleanup_failed"], true);
}

#[test]
fn close_joins_pending_work_before_releasing_ownership() {
    let mut reply = Reply::text("late");
    reply.header_delay = Duration::from_millis(1200);
    let server = Server::start(vec![reply]);
    assert_eq!(probe(&server, "shutdown", None)["state"], "cancelled");
}

#[test]
fn model_preparation_validates_config_without_resolving_credentials() {
    assert!(ModelProfile::new("default", json!({"model":""})).is_err());
    ModelProfile::new(
        "default",
        json!({"model":"example", "api_key_env":"MF_CHAT_MISSING_PREPARATION_KEY"}),
    )
    .unwrap();
}
