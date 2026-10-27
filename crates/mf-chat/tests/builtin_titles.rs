use mf_chat::{ChatHost, HostConfig, WorkflowProfile};
use mf_runtime::{WorkflowArguments, WorkflowDefinition};
use mfn_llm::test_support::{Reply, Server, messages};
use serde_json::json;
use std::time::Duration;

fn host(directory: &std::path::Path, server: &Server, enabled: bool) -> ChatHost {
    ChatHost::open(directory.join("chat.sqlite"), config(server, enabled)).unwrap()
}

fn config(server: &Server, enabled: bool) -> HostConfig {
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
    config
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

#[test]
fn builtin_title_is_a_registered_call_in_the_application_graph() {
    let authored =
        WorkflowDefinition::from_json(include_str!("../workflows/conversation.json")).unwrap();
    assert_eq!(
        authored.flows["title"],
        mf_runtime::FlowSource::Builtin {
            builtin: "mf-chat.chat-title".into()
        }
    );
    let plan = mf_chat::flows::conversation::plan().unwrap();
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
    assert!(
        body.nodes
            .iter()
            .any(|node| node.id.as_str() == "title_model" && node.kind == "builtin.llm")
    );
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    assert!(description.body(&["title".into()]).is_some());
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let manifest = mf_chat::flows::conversation::manifest(&registry).unwrap();
    assert!(manifest.description().unwrap().is_streaming());
    assert_eq!(
        manifest
            .input_schema
            .inputs
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["begin"]
    );
    let stream = mf_chat::flows::conversation::prepare_stream(&registry).unwrap();
    assert_eq!(stream.plan().input_schema(), &manifest.input_schema);
    let error = mf_chat::flows::conversation::prepare_workflow(&registry, None)
        .err()
        .unwrap();
    assert!(matches!(error, mf_chat::ChatError::Compile { source }
        if matches!(*source, mf_compiler::WorkflowCompileError::InvalidStream { .. })));
}

fn log_value(value: &opentelemetry::logs::AnyValue) -> serde_json::Value {
    use opentelemetry::logs::AnyValue;
    match value {
        AnyValue::String(value) => json!(value.as_str()),
        AnyValue::Int(value) => json!(value),
        AnyValue::Boolean(value) => json!(value),
        AnyValue::ListAny(values) => values.iter().map(log_value).collect(),
        AnyValue::Map(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.as_str().into(), log_value(value)))
                .collect(),
        ),
        _ => panic!("unexpected lifecycle value"),
    }
}

fn attribute(record: &opentelemetry_sdk::logs::SdkLogRecord, name: &str) -> serde_json::Value {
    record
        .attributes_iter()
        .find(|(key, _)| key.as_str() == name)
        .map(|(_, value)| log_value(value))
        .unwrap_or_default()
}

#[test]
fn builtin_title_lifecycle_shares_the_application_run_and_call_scope() {
    use opentelemetry_sdk::{
        logs::{InMemoryLogExporter, SdkLoggerProvider},
        trace::{InMemorySpanExporter, SdkTracerProvider},
    };
    let exporter = InMemoryLogExporter::default();
    let logs = SdkLoggerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let span_exporter = InMemorySpanExporter::default();
    let traces = SdkTracerProvider::builder()
        .with_simple_exporter(span_exporter.clone())
        .build();
    for enabled in [true, false] {
        exporter.reset();
        span_exporter.reset();
        let server = Server::start(if enabled {
            vec![Reply::text("Answer"), Reply::text("Title")]
        } else {
            vec![Reply::text("Answer")]
        });
        let root = tempfile::tempdir().unwrap();
        let mut config = config(&server, enabled);
        config.observer = Some(mf_telemetry::observation::Observer::new(&traces, &logs));
        let host = ChatHost::open(root.path().join("chat.sqlite"), config).unwrap();
        let session = host
            .database()
            .create_session("default", None, None)
            .unwrap();
        let result = host.submit(&session.id, "Prompt").unwrap().join().unwrap();
        host.close().unwrap();
        let records = exporter.get_emitted_logs().unwrap();
        let application = records
            .iter()
            .find(|log| {
                log.record.event_name() == Some("mf.node.started")
                    && attribute(&log.record, "mf.node.id") == "workflow"
            })
            .unwrap();
        let calls: Vec<_> = records
            .iter()
            .filter(|log| log.record.event_name() == Some("mf.call.started"))
            .collect();
        assert_eq!(calls.len(), usize::from(enabled));
        if !enabled {
            continue;
        }
        let call = &calls[0].record;
        let run_id = attribute(&application.record, "mf.run.id");
        assert_eq!(attribute(call, "mf.run.id"), run_id);
        assert_eq!(
            log_value(call.body().unwrap())["path"],
            json!([{"call_id":"title"}])
        );
        let started = records
            .iter()
            .position(|log| log.record.event_name() == Some("mf.call.started"))
            .unwrap();
        let finished = records
            .iter()
            .position(|log| log.record.event_name() == Some("mf.call.finished"))
            .unwrap();
        let closed = records
            .iter()
            .position(|log| {
                log.record.event_name() == Some("mf.node.finished")
                    && attribute(&log.record, "mf.node.id") == "workflow"
            })
            .unwrap();
        let settled = records
            .iter()
            .position(|log| {
                log.record.event_name() == Some("mf.node.started")
                    && attribute(&log.record, "mf.node.id") == "settle"
            })
            .unwrap();
        assert!(closed < started && started < finished && finished < settled);
        let title = records
            .iter()
            .find(|log| {
                log.record.event_name() == Some("mf.node.started")
                    && attribute(&log.record, "mf.node.id") == "title_model"
            })
            .unwrap();
        assert_eq!(attribute(&title.record, "mf.run.id"), run_id);
        assert_eq!(
            log_value(title.record.body().unwrap())["loop_path"],
            json!([{"call_id":"title"}])
        );
        let usage = result.result.usages.last().unwrap();
        assert_eq!(usage.node.as_ref().unwrap().path[0].node, "title");
        traces.force_flush().unwrap();
        let spans = span_exporter.get_finished_spans().unwrap();
        let node_span = |id, kind| {
            spans
                .iter()
                .find(|span| {
                    span.attributes.iter().any(|attribute| {
                        attribute.key.as_str() == "mf.node.id" && attribute.value.as_str() == id
                    }) && span.attributes.iter().any(|attribute| {
                        attribute.key.as_str() == "mf.node.kind" && attribute.value.as_str() == kind
                    })
                })
                .unwrap()
        };
        let parent = node_span("title", "workflow.call");
        let child = node_span("title_model", "builtin.llm");
        assert_eq!(child.parent_span_id, parent.span_context.span_id());
        assert_eq!(
            child.span_context.trace_id(),
            parent.span_context.trace_id()
        );
    }
}
