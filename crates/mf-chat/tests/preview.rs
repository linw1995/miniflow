use mf_chat::{
    ChatError, ChatHost, ConversationBackend, ConversationInputs, ConversationResult, HostConfig,
    Invocation, WorkflowProfile,
};
use mf_runtime::{ExecutionContext, StreamCancellation, StreamError};
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
        preview: Option<&mut dyn FnMut(mf_chat::TextDelta) -> Result<(), ChatError>>,
        _: Option<&mf_telemetry::observation::Observer>,
    ) -> Result<ConversationResult, ChatError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.cancellation.lock().unwrap() = Some(context.cancellation());
        if let Some(consume) = preview {
            for text in ["provisional", "-tail"] {
                consume(mf_chat::TextDelta {
                    text: text.into(),
                    model_call_id: "model-call".into(),
                })?;
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
    let (job, receiver) = host.submit_stream(&session, "hello").unwrap();
    let mut events = Vec::new();
    while let Ok(event) = receiver.recv() {
        events.push(event);
    }
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].delta.text, "provisional");
    assert_eq!(&events[0].invocation, job.invocation());
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
        let (job, receiver) = host.submit_stream(&session, "hello").unwrap();
        receiver.recv().unwrap();
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
    let (job, receiver) = host.submit_stream(&session, "hello").unwrap();
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
        input: Self::Input,
        context: &mut ExecutionContext,
        emitter: &mut mf_runtime::TypedEmitter<'_, '_, Emission>,
    ) -> Result<mf_runtime::TypedNodeResult<ConversationResult>, mf_runtime::NodeExecutionError>
    {
        assert!(emitter.is_connected());
        assert!(context.output("begin.database_path").is_err());
        if input.prompt != "legacy" {
            assert_eq!(
                context.invocation_path(),
                vec![mf_telemetry::event::ScopePathEntry::Call {
                    call_id: "workflow".into(),
                }]
            );
        }
        let call_id = uuid::Uuid::new_v4().to_string();
        let usage = mfn_llm::CallUsage {
            attribution: None,
            started_at: None,
            call_id: call_id.clone(),
            node: None,
            model: "embedded-model".into(),
            usage: Some(std::collections::BTreeMap::from([(
                "output_tokens".into(),
                2,
            )])),
            error: None,
        };
        mf_telemetry::events::record(mfn_llm::REQUEST_STARTED_EVENT, serde_json::json!(&usage));
        mf_telemetry::events::record(
            mfn_llm::FIRST_TEXT_EVENT,
            serde_json::json!({"call_id":call_id}),
        );
        emitter.send(Emission {
            delta: mf_chat::TextDelta {
                text: "prefix".into(),
                model_call_id: call_id,
            },
        })?;
        if input.prompt == "wait" {
            return Err(context
                .execute_future(context.cancellation().cancelled(), None)?
                .into());
        }
        mf_telemetry::events::record(mfn_llm::REQUEST_FINISHED_EVENT, serde_json::json!(&usage));
        Ok(ConversationResult {
            text: "isolated-final".into(),
            usages: vec![usage.into()],
            ..Default::default()
        }
        .into())
    }
}

fn embedded(_: serde_json::Value) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    mf_runtime::PreparedNode::typed_task_stream(Embedded, mf_runtime::NodeMetadata::default())
}
inventory::submit! { mf_runtime::NodeRegistration { kind: "test.chat.streaming", factory: mf_runtime::NodeFactory::Plain(embedded) } }

fn embedded_profile() -> WorkflowProfile {
    let definition = mf_compiler::WorkflowDefinition::from_json(&serde_json::json!({
        "version":"2026-10-23", "dependencies":{},
        "nodes":[{"id":"begin","kind":"test.chat.streaming"}],
        "outputs":[{"name":"answer","node":"begin","port":"delta"},{"name":"status","node":"begin","port":"status"},{"name":"text","node":"begin","port":"text"},{"name":"usages","node":"begin","port":"usages"}]
    }).to_string()).unwrap();
    WorkflowProfile::new("embedded", definition, Default::default()).unwrap()
}

#[test]
fn embedded_exports_preserve_user_identity_and_hide_colliding_application_context() {
    let root = tempfile::tempdir().unwrap();
    let profile = embedded_profile();
    let host = ChatHost::open(root.path().join("chat.sqlite"), HostConfig::new(profile)).unwrap();
    let session = host
        .database()
        .create_session("embedded", None, None)
        .unwrap();
    let (job, receiver) = host.submit_stream(&session.id, "hello").unwrap();
    let mut values = 0;
    let mut model_call = None;
    while let Ok(event) = receiver.recv() {
        assert_eq!(event.invocation.session_id, session.id);
        model_call = Some(event.delta.model_call_id);
        values += 1;
    }
    let completion = job.join().unwrap();
    assert_eq!(values, 1);
    assert_eq!(completion.result.text, "isolated-final");
    let usage = &completion.result.usages[0];
    assert_eq!(Some(&usage.call_id), model_call.as_ref());
    assert!(usage.timing.as_ref().unwrap().first_token_us.is_some());
}

#[test]
fn embedded_call_without_a_subscription_joins_with_its_terminal_result() {
    let root = tempfile::tempdir().unwrap();
    let host = ChatHost::open(
        root.path().join("chat.sqlite"),
        HostConfig::new(embedded_profile()),
    )
    .unwrap();
    let session = host
        .database()
        .create_session("embedded", None, None)
        .unwrap();
    let completion = host.submit(&session.id, "hello").unwrap().join().unwrap();
    assert_eq!(completion.result.text, "isolated-final");
    assert_eq!(completion.result.usages.len(), 1);
    assert_eq!(
        host.database().history(&session.id).unwrap().history.len(),
        2
    );
    host.close().unwrap();
}

#[test]
fn embedded_call_drains_public_streams_without_an_answer_export() {
    let original = embedded_profile();
    let backend = original.backend();
    let mut definition = backend.embedded_workflow().unwrap().0.definition.clone();
    definition
        .outputs
        .iter_mut()
        .find(|output| output.name == "answer")
        .unwrap()
        .name = "diagnostics".into();
    let profile = WorkflowProfile::new("embedded", definition, Default::default()).unwrap();
    assert!(profile.backend().embedded_workflow().is_some());
    let root = tempfile::tempdir().unwrap();
    let host = ChatHost::open(root.path().join("chat.sqlite"), HostConfig::new(profile)).unwrap();
    let session = host
        .database()
        .create_session("embedded", None, None)
        .unwrap();
    let completion = host.submit(&session.id, "hello").unwrap().join().unwrap();
    assert_eq!(completion.result.text, "isolated-final");
    assert_eq!(completion.result.usages.len(), 1);
    host.close().unwrap();
}

#[test]
fn direct_embedded_invocation_reuses_the_call_stream_interface() {
    let profile = embedded_profile();
    let mut deltas = Vec::new();
    let result = profile
        .backend()
        .invoke(
            &Invocation::new("session"),
            ConversationInputs {
                prompt: "hello".into(),
                system: None,
                history: None,
                context: None,
            },
            &mut ExecutionContext::default(),
            Some(&mut |delta| {
                deltas.push(delta);
                Ok(())
            }),
            Some(&mf_telemetry::observation::Observer::stderr(0)),
        )
        .unwrap();
    assert_eq!(result.text, "isolated-final");
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].text, "prefix");
    assert_eq!(deltas[0].model_call_id, result.usages[0].call_id);
}

#[test]
fn embedded_call_delivers_a_prefix_before_completion_and_shares_cancellation() {
    let root = tempfile::tempdir().unwrap();
    let host = ChatHost::open(
        root.path().join("chat.sqlite"),
        HostConfig::new(embedded_profile()),
    )
    .unwrap();
    let session = host
        .database()
        .create_session("embedded", None, None)
        .unwrap();
    let (job, receiver) = host.submit_stream(&session.id, "wait").unwrap();
    let invocation = job.invocation().clone();
    assert_eq!(receiver.recv().unwrap().delta.text, "prefix");
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

struct PreviewEventNode;

impl mf_runtime::TypedEventNode for PreviewEventNode {
    type Input = Emission;
    type Output = Emission;

    fn on_event(
        &mut self,
        event: mf_runtime::NodeEvent<Self::Input>,
        _: &mf_runtime::EventContext<'_>,
    ) -> Result<mf_runtime::EventEffects<Self::Output>, mf_runtime::NodeExecutionError> {
        Ok(mf_runtime::EventEffects {
            emissions: match event {
                mf_runtime::NodeEvent::Input(input) => {
                    vec![mf_runtime::TypedNodeResult::from(input).into()]
                }
                _ => vec![],
            },
            ..Default::default()
        })
    }
}

inventory::submit! {
    mf_runtime::NodeRegistration {
        kind: "test.chat.preview_event",
        factory: mf_runtime::NodeFactory::Plain(|_| {
            mf_runtime::PreparedNode::typed_event(PreviewEventNode, mf_runtime::NodeMetadata::default())
        }),
    }
}

#[test]
fn embedded_native_stream_graphs_keep_their_coordinator() {
    let definition = serde_json::from_value(serde_json::json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[
            {"id":"begin", "kind":"test.chat.streaming"},
            {"id":"preview", "kind":"test.chat.preview_event", "inputs":{
                "delta":{"from":{"node":"begin","port":"delta"}}
            }}
        ],
        "outputs":[
            {"name":"answer","node":"preview","port":"delta","optional":true},
            {"name":"status","node":"begin","port":"status"},
            {"name":"text","node":"begin","port":"text"},
            {"name":"usages","node":"begin","port":"usages"}
        ]
    }))
    .unwrap();
    let profile = WorkflowProfile::new("embedded", definition, Default::default()).unwrap();
    assert!(profile.backend().embedded_workflow().is_none());
    let root = tempfile::tempdir().unwrap();
    let host = ChatHost::open(root.path().join("chat.sqlite"), HostConfig::new(profile)).unwrap();
    let session = host
        .database()
        .create_session("embedded", None, None)
        .unwrap();
    let (job, receiver) = host.submit_stream(&session.id, "legacy").unwrap();
    assert_eq!(receiver.recv().unwrap().delta.text, "prefix");
    assert!(receiver.recv().is_err());
    assert_eq!(job.join().unwrap().result.text, "isolated-final");
    host.close().unwrap();
}

#[test]
fn pre_cancelled_requests_do_not_begin_application_or_embedded_flows() {
    let root = tempfile::tempdir().unwrap();
    let profile = embedded_profile();
    let backend = profile.backend();
    let host = ChatHost::open(root.path().join("chat.sqlite"), HostConfig::new(profile)).unwrap();
    let database = host.database();
    let session = database.create_session("embedded", None, None).unwrap();
    let invocation = Invocation::new(&session.id);
    let cancellation = StreamCancellation::default();
    cancellation.cancel(
        mf_runtime::StreamExecutionSnafu {
            message: "cancelled before dispatch",
        }
        .build(),
    );
    let result = mf_chat::workflow::run_conversation(
        database,
        mf_chat::nodes::BeginTurnInputs {
            database_path: database.path().to_str().unwrap().into(),
            session_id: session.id,
            turn_id: invocation.turn_id.clone(),
            call_id: invocation.call_id.clone(),
            prompt: "hello".into(),
            model: mf_chat::ModelIdentity {
                profile: "embedded".into(),
                model: None,
            },
            retry_of: None,
        },
        backend.clone(),
        cancellation.clone(),
        false,
        None,
    );
    assert!(matches!(result,
        Err(ChatError::Stream { source: StreamError::Execution { message } })
            if message == "cancelled before dispatch"
    ));
    assert!(database.turn(&invocation.turn_id).is_err());

    let mut context = ExecutionContext::default();
    context.set_cancellation(cancellation);
    let result = backend.invoke(
        &invocation,
        ConversationInputs {
            prompt: "hello".into(),
            system: None,
            history: None,
            context: None,
        },
        &mut context,
        Some(&mut |_| panic!("a cancelled request must not deliver previews")),
        None,
    );
    assert!(matches!(result,
        Err(ChatError::Stream { source: StreamError::Execution { message } })
            if message == "cancelled before dispatch"
    ));
}

#[test]
fn nonblocking_preview_consumption_releases_native_settlement() {
    let (_root, host, session, _) = setup();
    let (mut job, receiver) = host.submit_stream(&session, "hello").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    loop {
        match receiver.try_recv() {
            Ok(event) => events.push(event),
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
        }
        if job.poll() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(events.len(), 2);
    assert_eq!(job.join().unwrap().result.text, "canonical");
}

struct RetryBackend(AtomicUsize);

impl ConversationBackend for RetryBackend {
    fn invoke(
        &self,
        _: &Invocation,
        _: ConversationInputs,
        _: &mut ExecutionContext,
        preview: Option<&mut dyn FnMut(mf_chat::TextDelta) -> Result<(), ChatError>>,
        _: Option<&mf_telemetry::observation::Observer>,
    ) -> Result<ConversationResult, ChatError> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            return mf_chat::CaptureSnafu {
                message: "injected backend failure",
            }
            .fail();
        }
        if let Some(consume) = preview {
            consume(mf_chat::TextDelta {
                text: "retried".into(),
                model_call_id: "retry-model".into(),
            })?;
        }
        Ok(ConversationResult {
            text: "retried-final".into(),
            ..Default::default()
        })
    }
}

#[test]
fn streaming_retries_use_new_invocations_and_preserve_failed_history() {
    let root = tempfile::tempdir().unwrap();
    let backend = Arc::new(RetryBackend(AtomicUsize::new(0)));
    let profile = WorkflowProfile::with_backend("retry", backend.clone()).unwrap();
    let host = ChatHost::open(root.path().join("chat.sqlite"), HostConfig::new(profile)).unwrap();
    let session = host.database().create_session("retry", None, None).unwrap();
    let original = host.submit(&session.id, "hello").unwrap();
    let failed = original.invocation().clone();
    assert!(original.join().is_err());
    assert!(
        host.database()
            .history(&session.id)
            .unwrap()
            .history
            .is_empty()
    );
    let (retry, receiver) = host.retry_stream(&failed.turn_id).unwrap();
    assert_ne!(retry.invocation().turn_id, failed.turn_id);
    let event = receiver.recv().unwrap();
    assert_eq!(&event.invocation, retry.invocation());
    assert_eq!(event.delta.text, "retried");
    assert!(receiver.recv().is_err());
    assert_eq!(retry.join().unwrap().result.text, "retried-final");
    assert_eq!(
        host.database().turn(&failed.turn_id).unwrap().state,
        "failed"
    );
    assert_eq!(
        host.database().history(&session.id).unwrap().history.len(),
        2
    );
    assert_eq!(backend.0.load(Ordering::SeqCst), 2);
}
