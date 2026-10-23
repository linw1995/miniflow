use mf_chat::{
    ChatError, ChatHost, ConversationBackend, ConversationInputs, ConversationResult, HostConfig,
    Invocation, WorkflowProfile,
};
use mf_runtime::{
    ExecutionContext, LiveOutputEvent, LiveOutputKind, StreamCancellation, StreamInvocation,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

struct Backend {
    calls: Arc<AtomicUsize>,
    returned: Arc<AtomicBool>,
    cancellation: Arc<Mutex<Option<StreamCancellation>>>,
}

impl ConversationBackend for Backend {
    fn invoke(
        &self,
        _: &Invocation,
        _: ConversationInputs,
        context: &mut ExecutionContext,
        _: Option<&mf_telemetry::observation::Observer>,
    ) -> Result<ConversationResult, ChatError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.cancellation.lock().unwrap() = Some(context.cancellation());
        if let Some(publisher) = context.live_publisher() {
            for event in [
                LiveOutputKind::Open,
                LiveOutputKind::Value {
                    value: serde_json::json!({"text":"provisional","model_call_id":"model-call"})
                        .into(),
                },
                LiveOutputKind::Close { completed: true },
            ] {
                publisher.publish(
                    LiveOutputEvent {
                        export: "answer".into(),
                        source: vec!["answer".into()],
                        invocation: StreamInvocation {
                            id: 1,
                            node: "answer".into(),
                            path: vec![],
                        },
                        event,
                    },
                    &context.cancellation(),
                )?;
            }
        }
        self.returned.store(true, Ordering::Release);
        Ok(ConversationResult {
            text: "canonical".into(),
            ..Default::default()
        })
    }
}

fn setup() -> (tempfile::TempDir, ChatHost, String, Arc<Backend>) {
    let root = tempfile::tempdir().unwrap();
    let backend = Arc::new(Backend {
        calls: Arc::default(),
        returned: Arc::default(),
        cancellation: Arc::default(),
    });
    let profile = WorkflowProfile::with_backend("fixture", backend.clone()).unwrap();
    let host = ChatHost::open(root.path().join("chat.sqlite"), HostConfig::new(profile)).unwrap();
    let session = host
        .database()
        .create_session("fixture", None, None)
        .unwrap();
    (root, host, session.id, backend)
}

#[test]
fn terminal_only_jobs_join_without_an_unread_receiver() {
    let (_root, host, session, backend) = setup();
    let result = host.submit(&session, "hello").unwrap().join().unwrap();
    assert_eq!(result.result.text, "canonical");
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert!(
        backend
            .cancellation
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .failure()
            .is_none()
    );
}

#[test]
fn explicit_subscription_precedes_launch_and_completion_does_not_cancel_closed_output() {
    let (_root, host, session, backend) = setup();
    let (job, receiver) = host
        .submit_stream(
            &session,
            "hello",
            mf_runtime::LiveLimits {
                max_pending_records: 1,
                ..Default::default()
            },
        )
        .unwrap();
    let mut events = Vec::new();
    while let Some(event) = receiver.recv().unwrap() {
        events.push(event);
    }
    assert_eq!(events.len(), 3);
    assert!(matches!(events[0].event, LiveOutputKind::Open));
    let result = job.join().unwrap();
    assert_eq!(result.result.text, "canonical");
    assert!(
        backend
            .cancellation
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .failure()
            .is_none()
    );
}

#[test]
fn dropped_subscription_and_host_shutdown_interrupt_pressure() {
    for close in [false, true] {
        let (_root, host, session, _) = setup();
        let (job, receiver) = host
            .submit_stream(
                &session,
                "hello",
                mf_runtime::LiveLimits {
                    max_pending_records: 1,
                    ..Default::default()
                },
            )
            .unwrap();
        receiver.recv().unwrap().unwrap();
        if close {
            let started = Instant::now();
            host.close().unwrap();
            assert!(started.elapsed() < Duration::from_secs(2));
        } else {
            drop(receiver);
        }
        assert!(job.join().is_err());
    }
}

#[test]
fn failed_final_drain_retains_authoritative_reply_for_save_only_retry() {
    let (_root, host, session, backend) = setup();
    let (job, receiver) = host
        .submit_stream(&session, "hello", Default::default())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !backend.returned.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    // All publications succeeded; only the post-result drain can now fail.
    drop(receiver);
    let ChatError::Uncommitted { pending, .. } = job.join().unwrap_err() else {
        panic!("expected retained authoritative reply")
    };
    assert_eq!(pending.completion.result.text, "canonical");
    let saved = host.retry_completion(*pending).unwrap();
    assert_eq!(saved.result.text, "canonical");
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

#[derive(mf_runtime::NodeValue)]
struct Emission {
    delta: mf_chat::TextDelta,
}

struct Embedded;
impl mf_runtime::TypedTaskStreamNode for Embedded {
    type Input = ConversationInputs;
    type Output = ConversationResult;
    type Emission = Emission;
    fn execute(
        &self,
        _: Self::Input,
        context: &mut ExecutionContext,
        emitter: &mut mf_runtime::TypedEmitter<'_, '_, Emission>,
    ) -> Result<mf_runtime::TypedNodeResult<ConversationResult>, mf_runtime::NodeExecutionError>
    {
        assert!(context.output("begin.database_path").is_err());
        emitter.send(Emission {
            delta: mf_chat::TextDelta {
                text: "prefix".into(),
                model_call_id: "embedded-model-call".into(),
            },
        })?;
        Ok(ConversationResult {
            text: "isolated-final".into(),
            ..Default::default()
        }
        .into())
    }
}

fn embedded(_: serde_json::Value) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    mf_runtime::PreparedNode::typed_task_stream(Embedded, mf_runtime::NodeMetadata::default())
}
inventory::submit! { mf_runtime::NodeRegistration { kind: "test.chat.live", factory: mf_runtime::NodeFactory::Plain(embedded) } }

#[test]
fn embedded_exports_preserve_user_identity_and_hide_colliding_application_context() {
    let root = tempfile::tempdir().unwrap();
    let definition = mf_compiler::WorkflowDefinition::from_json(&serde_json::json!({
        "version":"2026-10-23", "dependencies":{},
        "nodes":[{"id":"begin","kind":"test.chat.live"}],
        "outputs":[{"name":"status","node":"begin","port":"status"},{"name":"text","node":"begin","port":"text"},{"name":"usages","node":"begin","port":"usages"}],
        "stream_outputs":[{"name":"answer","path":["begin"],"port":"delta"}]
    }).to_string()).unwrap();
    let profile = WorkflowProfile::new("embedded", definition, Default::default()).unwrap();
    let host = ChatHost::open(root.path().join("chat.sqlite"), HostConfig::new(profile)).unwrap();
    let session = host
        .database()
        .create_session("embedded", None, None)
        .unwrap();
    let (job, receiver) = host
        .submit_stream(&session.id, "hello", Default::default())
        .unwrap();
    let mut values = 0;
    while let Some(event) = receiver.recv().unwrap() {
        assert_eq!(event.source, [mf_runtime::DefinitionId::from("begin")]);
        assert_eq!(event.invocation.node, "begin");
        if matches!(event.event, LiveOutputKind::Value { .. }) {
            values += 1;
        }
    }
    assert_eq!(values, 1);
    assert_eq!(job.join().unwrap().result.text, "isolated-final");
}
