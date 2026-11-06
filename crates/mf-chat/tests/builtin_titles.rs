use mf_chat::{ChatHost, HostConfig, WorkflowProfile};
use mf_runtime::{WorkflowArguments, WorkflowDefinition};
use mfn_llm::test_support::{Reply, Server, messages};
use serde_json::json;
use std::time::Duration;

fn host(directory: &std::path::Path, server: &Server, enabled: bool) -> ChatHost {
    let definition =
        WorkflowDefinition::from_json(include_str!("../../../examples/chat-flow.json")).unwrap();
    let profile = WorkflowProfile::new(
        "default",
        definition,
        WorkflowArguments::try_from(json!({
            "params.model":"fixture-model",
            "params.base_url":server.base_url,
            "params.api_key":"synthetic-key"
        }))
        .unwrap(),
    )
    .unwrap();
    let mut config = HostConfig::new(profile);
    config.automatic_titles = enabled;
    ChatHost::open(directory.join("chat.sqlite"), config).unwrap()
}

#[test]
fn builtin_title_runs_once_and_persists_usage_with_the_answer() {
    for failed in [false, true] {
        let title = if failed {
            let mut reply = Reply::json(json!({"error":{"message":"title failed"}}));
            reply.status = 400;
            reply
        } else {
            Reply::text("Generated title")
        };
        let server = Server::start(vec![
            Reply::text("First answer"),
            title,
            Reply::text("Second answer"),
        ]);
        let root = tempfile::tempdir().unwrap();
        let host = host(root.path(), &server, true);
        let session = host
            .database()
            .create_session("default", None, None)
            .unwrap();
        let first = host
            .submit(&session.id, "First prompt")
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(first.result.text, "First answer");
        assert_eq!(
            first.result.title.as_deref(),
            (!failed).then_some("Generated title")
        );
        assert_eq!(first.result.usages.len(), 2);
        let conversation = first.result.usages[0].node.as_ref().unwrap();
        assert_eq!(conversation.id, "chat");
        assert_eq!(
            serde_json::to_value(&conversation.path).unwrap(),
            json!([{"node":"workflow","index":0}])
        );
        assert_eq!(
            host.database().session(&session.id).unwrap().title,
            if failed {
                session.title.clone()
            } else {
                "Generated title".into()
            }
        );
        let usage = &first.result.usages[1];
        assert_eq!(
            usage.attribution.as_ref().unwrap().purpose.as_deref(),
            Some("title")
        );
        assert_eq!(usage.error.is_some(), failed);
        let node = usage.node.as_ref().unwrap();
        assert_eq!(node.id, "title_model");
        assert_eq!(
            serde_json::to_value(&node.path).unwrap(),
            json!([{"node":"title","index":0}])
        );
        assert!(
            usage
                .timing
                .as_ref()
                .is_some_and(|timing| timing.is_valid())
        );
        let second = host
            .submit(&session.id, "Second prompt")
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(second.result.text, "Second answer");
        assert_eq!(second.result.usages.len(), 1);
        assert!(second.result.title.is_none());
        assert_eq!(
            host.database().history(&session.id).unwrap().history.len(),
            4
        );
        assert_eq!(
            host.database()
                .usage(Default::default())
                .unwrap()
                .overall
                .attempted_calls,
            3
        );
        assert_eq!(
            host.database().session(&session.id).unwrap().title_source,
            if failed { "provisional" } else { "generated" }
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 3);
        assert!(
            messages(&requests[1])[1]["content"]
                .as_str()
                .unwrap()
                .contains("assistant: First answer")
        );
        assert!(
            requests
                .iter()
                .all(|request| request.body["model"] == "fixture-model")
        );
        host.close().unwrap();
    }
}

#[test]
fn builtin_title_respects_disabled_policy_manual_titles_and_failed_answers() {
    for scenario in ["disabled", "manual", "failed"] {
        let mut reply = Reply::text("Answer");
        if scenario == "failed" {
            reply = Reply::json(json!({"error":{"message":"answer failed"}}));
            reply.status = 400;
        }
        let server = Server::start(vec![reply]);
        let root = tempfile::tempdir().unwrap();
        let host = host(root.path(), &server, scenario != "disabled");
        let session = host
            .database()
            .create_session(
                "default",
                None,
                (scenario == "manual").then(|| "Manual".into()),
            )
            .unwrap();
        let result = host.submit(&session.id, "Prompt").unwrap().join();
        assert_eq!(result.is_err(), scenario == "failed");
        assert_eq!(
            host.database().session(&session.id).unwrap().title,
            session.title
        );
        assert_eq!(server.requests().len(), 1);
        host.close().unwrap();
    }
}

#[test]
fn cancelling_the_builtin_title_does_not_commit_an_intermediate_answer() {
    let mut title = Reply::text("Generated title");
    title.header_delay = Duration::from_millis(1200);
    let server = Server::start(vec![Reply::text("Answer"), title]);
    let root = tempfile::tempdir().unwrap();
    let host = host(root.path(), &server, true);
    let session = host
        .database()
        .create_session("default", None, None)
        .unwrap();
    let job = host.submit(&session.id, "Prompt").unwrap();
    let invocation = job.invocation().clone();
    server.wait_for_requests(2);
    job.cancel();
    assert!(job.join().is_err());
    assert_eq!(
        host.database().turn(&invocation.turn_id).unwrap().state,
        "cancelled"
    );
    assert!(
        host.database()
            .history(&session.id)
            .unwrap()
            .history
            .is_empty()
    );
    host.close().unwrap();
}

#[test]
fn conversation_finish_retains_the_title_for_save_only_retry() {
    use diesel::{Connection, connection::SimpleConnection};
    let server = Server::start(vec![Reply::text("Answer"), Reply::text("Generated title")]);
    let root = tempfile::tempdir().unwrap();
    let host = host(root.path(), &server, true);
    let session = host
        .database()
        .create_session("default", None, None)
        .unwrap();
    let mut connection =
        diesel::SqliteConnection::establish(host.database().path().to_str().unwrap()).unwrap();
    connection.batch_execute("CREATE TRIGGER reject_title BEFORE UPDATE ON sessions WHEN NEW.title_source = 'generated' BEGIN SELECT RAISE(ABORT, 'injected title write failure'); END;").unwrap();
    let error = host
        .submit(&session.id, "Prompt")
        .unwrap()
        .join()
        .unwrap_err();
    let mf_chat::ChatError::Uncommitted { pending, .. } = error else {
        panic!("expected the complete conversation to remain recoverable")
    };
    assert_eq!(
        pending.completion.result.title.as_deref(),
        Some("Generated title")
    );
    assert_eq!(pending.completion.result.usages.len(), 2);
    assert_eq!(
        host.database().session(&session.id).unwrap().title,
        session.title
    );
    assert!(
        host.database()
            .history(&session.id)
            .unwrap()
            .history
            .is_empty()
    );
    connection
        .batch_execute("DROP TRIGGER reject_title;")
        .unwrap();
    let saved = host.retry_completion(*pending).unwrap();
    assert_eq!(saved.result.text, "Answer");
    assert_eq!(
        host.database().session(&session.id).unwrap().title,
        "Generated title"
    );
    assert_eq!(
        host.database().history(&session.id).unwrap().history.len(),
        2
    );
    assert_eq!(server.requests().len(), 2);
    host.close().unwrap();
}

#[test]
fn embedded_application_declares_and_prepares_the_builtin_title_call() {
    let authored =
        WorkflowDefinition::from_json(include_str!("../workflows/conversation.json")).unwrap();
    assert_eq!(
        authored.flows["title"],
        mf_runtime::FlowSource::Builtin {
            builtin: "mf-chat.chat-title".into(),
            config: serde_json::Value::Null,
            flows: Default::default(),
        }
    );
    let plan = mf_chat::flows::conversation::plan().unwrap();
    for (id, kind) in [
        ("prepare_title", "mf_chat.title.prepare"),
        ("finish", "mf_chat.conversation.finish"),
    ] {
        assert!(
            plan.definition
                .nodes
                .iter()
                .any(|node| node.id.as_str() == id && node.kind == kind)
        );
    }
    let call = plan
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "title")
        .unwrap();
    assert_eq!(call.kind, mf_runtime::CALL_KIND);
    let configuration: mf_runtime::CallConfig =
        serde_json::from_value(call.config.clone()).unwrap();
    let body = configuration.definition.unwrap();
    let registered = mf_chat::flows::title_definition().unwrap();
    let registered_plan = mf_compiler::plan_definition(&registered).unwrap();
    assert_eq!(*body, registered_plan.definition);
}
