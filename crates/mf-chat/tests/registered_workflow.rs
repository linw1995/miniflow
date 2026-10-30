use mf_chat::{
    ChatError, ChatHost, ConversationBackend, ConversationInputs, ConversationResult, HostConfig,
    Invocation, WorkflowProfile,
};
use mf_runtime::{
    ExecutionContext, InputBinding, NodeValue, WorkflowArguments, WorkflowInput, WorkflowManifest,
};
use snafu::ResultExt;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Runner {
    manifest: WorkflowManifest,
    parameters: WorkflowArguments,
    calls: AtomicUsize,
    fail_after_reply: bool,
}

impl Runner {
    fn new(fail_after_reply: bool) -> Self {
        let outputs: Vec<_> = ConversationResult::ports().into_iter()
            .map(|port| serde_json::json!({
                "name":port.name,"node":"remote","port":port.name,"optional":!port.required
            })).chain([serde_json::json!({"name":"answer","node":"remote","port":"answer","optional":true})]).collect();
        Self {
            manifest: WorkflowManifest {
                version: mf_runtime::WorkflowManifestVersion::V2026_10_16,
                definition: serde_json::from_value(serde_json::json!({
                    "version":"2026-10-28", "dependencies":{},
                    "execution":{},
                    "nodes":[{"id":"remote","kind":"unlinked.chat_provider"}],
                    "outputs":outputs
                }))
                .unwrap(),
                execution_order: vec!["remote".into()],
                loop_orders: vec![],
                input_schema: mf_runtime::WorkflowInputSchema {
                    inputs: [(
                        "remote".into(),
                        ConversationInputs::ports()
                            .into_iter()
                            .map(|port| {
                                (
                                    port.name.to_string(),
                                    WorkflowInput {
                                        schema: None,
                                        value_type: port.value_type,
                                        required: port.required,
                                        binding: InputBinding::default(),
                                    },
                                )
                            })
                            .collect(),
                    )]
                    .into(),
                    stdin: Default::default(),
                },
            },
            parameters: Default::default(),
            calls: AtomicUsize::new(0),
            fail_after_reply,
        }
    }
}

impl ConversationBackend for Runner {
    fn registered_workflow(&self) -> Option<(&WorkflowManifest, &WorkflowArguments)> {
        Some((&self.manifest, &self.parameters))
    }

    fn invoke(
        &self,
        _: &Invocation,
        request: ConversationInputs,
        context: &mut ExecutionContext,
        preview: Option<&mut dyn FnMut(mf_chat::TextDelta) -> Result<(), ChatError>>,
        _: Option<&mf_telemetry::observation::Observer>,
    ) -> Result<ConversationResult, ChatError> {
        assert!(context.output("begin.database_path").is_err());
        assert_eq!(
            context.invocation_path(),
            [mf_telemetry::event::ScopePathEntry::Call {
                call_id: "workflow".into()
            }]
        );
        assert!(request.context.is_some());
        self.calls.fetch_add(1, Ordering::SeqCst);
        preview.expect("declared streams are drained even without a subscriber")(
            mf_chat::TextDelta {
                text: "preview".into(),
                model_call_id: "model".into(),
            },
        )?;
        let reply = ConversationResult {
            text: request.prompt,
            ..Default::default()
        };
        if self.fail_after_reply {
            return mf_chat::CaptureSnafu {
                message: "child exited after its final reply",
            }
            .fail()
            .boxed()
            .context(mf_chat::BackendSnafu {
                outcome: mf_chat::contract::TurnExit::Failed,
                reply: Some(Box::new(reply)),
            });
        }
        Ok(reply)
    }
}

fn setup(fail_after_reply: bool) -> (tempfile::TempDir, ChatHost, String, Arc<Runner>) {
    let root = tempfile::tempdir().unwrap();
    let runner = Arc::new(Runner::new(fail_after_reply));
    let profile = WorkflowProfile::with_backend("runner", runner.clone()).unwrap();
    let host = ChatHost::open(root.path().join("chat.sqlite"), HostConfig::new(profile)).unwrap();
    let session = host
        .database()
        .create_session("runner", None, None)
        .unwrap();
    (root, host, session.id, runner)
}

#[test]
fn registered_runner_streams_inside_a_call_without_linked_child_nodes() {
    let (_root, host, session, runner) = setup(false);
    let (job, receiver) = host.submit_stream(&session, "first").unwrap();
    assert_eq!(receiver.recv().unwrap().delta.text, "preview");
    assert!(receiver.recv().is_err());
    assert_eq!(job.join().unwrap().result.text, "first");
    assert_eq!(
        host.submit(&session, "second")
            .unwrap()
            .join()
            .unwrap()
            .result
            .text,
        "second"
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 2);
    assert_eq!(host.database().history(&session).unwrap().history.len(), 4);
}

#[test]
fn registered_runner_retains_final_reply_on_process_failure_without_reinvocation() {
    let (_root, host, session, runner) = setup(true);
    let error = host
        .submit(&session, "retained")
        .unwrap()
        .join()
        .unwrap_err();
    let ChatError::Uncommitted { pending, .. } = error else {
        panic!("expected retained reply, got {error:?}");
    };
    assert_eq!(pending.completion.result.text, "retained");
    host.database().save_recovered(*pending).unwrap();
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.database().history(&session).unwrap().history.len(), 2);
}
