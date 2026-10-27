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
        let usage = &first.result.usages[1];
        assert_eq!(
            usage.attribution.as_ref().unwrap().purpose.as_deref(),
            Some("title")
        );
        assert_eq!(usage.error.is_some(), failed);
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
            host.database()
                .history(&session.id, 4096)
                .unwrap()
                .history
                .len(),
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
fn builtin_title_preserves_concurrent_renames_and_cancellation_semantics() {
    for cancel in [false, true] {
        let mut title = Reply::text("Generated title");
        title.header_delay = Duration::from_millis(300);
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
        if cancel {
            job.cancel();
            assert!(job.join().is_err());
            assert_eq!(
                host.database().turn(&invocation.turn_id).unwrap().state,
                "cancelled"
            );
            assert!(
                host.database()
                    .history(&session.id, 4096)
                    .unwrap()
                    .history
                    .is_empty()
            );
        } else {
            host.database()
                .rename_session(&session.id, "Manual")
                .unwrap();
            let completion = job.join().unwrap();
            assert_eq!(completion.result.usages.len(), 2);
            assert_eq!(
                host.database().session(&session.id).unwrap().title,
                "Manual"
            );
        }
        host.close().unwrap();
    }
}
