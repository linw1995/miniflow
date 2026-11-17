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
        preview: Option<&mut dyn FnMut(mf_chat::PreviewDelta) -> Result<(), ChatError>>,
        _: Option<&mf_telemetry::observation::Observer>,
    ) -> Result<ConversationResult, ChatError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.cancellation.lock().unwrap() = Some(context.cancellation());
        if let Some(consume) = preview {
            for text in ["provisional", "-tail"] {
                consume(
                    mf_chat::TextDelta {
                        text: text.into(),
                        model_call_id: "model-call".into(),
                    }
                    .into(),
                )?;
            }
        }
        self.returned.store(true, Ordering::Release);
        Ok(ConversationResult::assistant("canonical"))
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
    assert_eq!(result.result.final_text(), Some("canonical"));
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
    assert!(
        matches!(&events[0].delta, mf_chat::PreviewDelta::Text { delta } if delta.text == "provisional")
    );
    assert_eq!(&events[0].invocation, job.invocation());
    let result = job.join().unwrap();
    assert_eq!(result.result.final_text(), Some("canonical"));
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
    assert_eq!(pending.completion.result.final_text(), Some("canonical"));
    let saved = host.retry_completion(*pending).unwrap();
    assert_eq!(saved.result.final_text(), Some("canonical"));
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
        if input.prompt()? != "legacy" {
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
        if input.prompt()? == "wait" {
            return Err(context
                .execute_future(context.cancellation().cancelled(), None)?
                .into());
        }
        mf_telemetry::events::record(mfn_llm::REQUEST_FINISHED_EVENT, serde_json::json!(&usage));
        Ok(ConversationResult {
            update: ConversationResult::assistant("isolated-final").update,
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
        "outputs":[{"name":"answer","node":"begin","port":"delta","optional":true},{"name":"append","node":"begin","port":"append"}]
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
        let mf_chat::PreviewDelta::Text { delta } = event.delta else {
            panic!("expected text")
        };
        model_call = Some(delta.model_call_id);
        values += 1;
    }
    let completion = job.join().unwrap();
    assert_eq!(values, 1);
    assert_eq!(completion.result.final_text(), Some("isolated-final"));
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
    assert_eq!(completion.result.final_text(), Some("isolated-final"));
    assert_eq!(completion.result.usages.len(), 1);
    assert_eq!(
        host.database()
            .model_context(&session.id)
            .unwrap()
            .messages
            .len(),
        2
    );
    host.close().unwrap();
}

#[test]
fn embedded_call_drains_public_streams_without_an_answer_export() {
    let original = embedded_profile();
    let backend = original.backend();
    let mut definition = backend
        .embedded_workflow()
        .unwrap()
        .0
        .graph
        .definition
        .clone();
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
    assert_eq!(completion.result.final_text(), Some("isolated-final"));
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
                messages: vec![mf_chat::ChatMessage::User {
                    content: "hello".into(),
                    metadata: None,
                }],
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
    assert_eq!(result.final_text(), Some("isolated-final"));
    assert_eq!(deltas.len(), 1);
    let mf_chat::PreviewDelta::Text { delta } = &deltas[0] else {
        panic!("expected text")
    };
    assert_eq!(delta.text, "prefix");
    assert_eq!(delta.model_call_id, result.usages[0].call_id);
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
    assert!(
        matches!(receiver.recv().unwrap().delta, mf_chat::PreviewDelta::Text { delta } if delta.text == "prefix")
    );
    job.cancel();
    assert!(job.join().is_err());
    assert_eq!(
        host.database().turn(&invocation.turn_id).unwrap().state,
        "cancelled"
    );
    assert!(
        host.database()
            .model_context(&session.id)
            .unwrap()
            .messages
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
            {"name":"append","node":"begin","port":"append"}
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
    assert!(
        matches!(receiver.recv().unwrap().delta, mf_chat::PreviewDelta::Text { delta } if delta.text == "prefix")
    );
    assert!(receiver.recv().is_err());
    assert_eq!(
        job.join().unwrap().result.final_text(),
        Some("isolated-final")
    );
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
            result_alternatives: None,
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
            messages: vec![mf_chat::ChatMessage::User {
                content: "hello".into(),
                metadata: None,
            }],
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
    assert_eq!(job.join().unwrap().result.final_text(), Some("canonical"));
}

struct RetryBackend(AtomicUsize);

impl ConversationBackend for RetryBackend {
    fn invoke(
        &self,
        _: &Invocation,
        _: ConversationInputs,
        _: &mut ExecutionContext,
        preview: Option<&mut dyn FnMut(mf_chat::PreviewDelta) -> Result<(), ChatError>>,
        _: Option<&mf_telemetry::observation::Observer>,
    ) -> Result<ConversationResult, ChatError> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            return mf_chat::CaptureSnafu {
                message: "injected backend failure",
            }
            .fail();
        }
        if let Some(consume) = preview {
            consume(
                mf_chat::TextDelta {
                    text: "retried".into(),
                    model_call_id: "retry-model".into(),
                }
                .into(),
            )?;
        }
        Ok(ConversationResult::assistant("retried-final"))
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
            .model_context(&session.id)
            .unwrap()
            .messages
            .is_empty()
    );
    let (retry, receiver) = host.retry_stream(&failed.turn_id).unwrap();
    assert_ne!(retry.invocation().turn_id, failed.turn_id);
    let event = receiver.recv().unwrap();
    assert_eq!(&event.invocation, retry.invocation());
    assert!(
        matches!(event.delta, mf_chat::PreviewDelta::Text { delta } if delta.text == "retried")
    );
    assert!(receiver.recv().is_err());
    assert_eq!(
        retry.join().unwrap().result.final_text(),
        Some("retried-final")
    );
    assert_eq!(
        host.database().turn(&failed.turn_id).unwrap().state,
        "failed"
    );
    assert_eq!(
        host.database()
            .model_context(&session.id)
            .unwrap()
            .messages
            .len(),
        2
    );
    assert_eq!(backend.0.load(Ordering::SeqCst), 2);
}
