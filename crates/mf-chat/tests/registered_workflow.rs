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
    tool_previews: bool,
}

impl Runner {
    fn new(fail_after_reply: bool) -> Self {
        let outputs: Vec<_> = ConversationResult::ports().into_iter()
            .map(|port| serde_json::json!({
                "name":port.name,"node":"remote","port":port.name,"optional":!port.required
            })).chain([serde_json::json!({"name":"answer","node":"remote","port":"answer","optional":true})]).collect();
        Self {
            manifest: WorkflowManifest::from_graph(
                &(mf_runtime::CompiledGraph {
                    definition: serde_json::from_value(serde_json::json!({
                        "version":"2026-10-28", "dependencies":{},
                        "execution":{},
                        "nodes":[{"id":"remote","kind":"unlinked.chat_provider"}],
                        "outputs":outputs
                    }))
                    .unwrap(),
                    execution_order: vec!["remote".into()],
                    relations: Default::default(),
                    children: Default::default(),
                }),
                mf_runtime::WorkflowInputSchema {
                    inputs: [(
                        "remote".into(),
                        ConversationInputs::ports()
                            .into_iter()
                            .map(|port| {
                                (
                                    port.name.to_string(),
                                    WorkflowInput {
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
            )
            .unwrap(),
            parameters: Default::default(),
            calls: AtomicUsize::new(0),
            fail_after_reply,
            tool_previews: false,
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
        preview: Option<&mut dyn FnMut(mf_chat::PreviewDelta) -> Result<(), ChatError>>,
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
        let consume = preview.expect("declared streams are drained even without a subscriber");
        if self.tool_previews {
            for arguments in ["{", "}"] {
                consume(
                    mf_chat::ToolCallDelta {
                        model_call_id: "model".into(),
                        index: 0,
                        arguments: arguments.into(),
                        id: None,
                        name: None,
                    }
                    .into(),
                )?;
            }
        } else {
            consume(
                mf_chat::TextDelta {
                    text: "preview".into(),
                    model_call_id: "model".into(),
                }
                .into(),
            )?;
        }
        let reply = ConversationResult::assistant(request.prompt()?);
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
    assert!(
        matches!(receiver.recv().unwrap().delta, mf_chat::PreviewDelta::Text { delta } if delta.text == "preview")
    );
    assert!(receiver.recv().is_err());
    assert_eq!(job.join().unwrap().result.final_text(), Some("first"));
    assert_eq!(
        host.submit(&session, "second")
            .unwrap()
            .join()
            .unwrap()
            .result
            .final_text(),
        Some("second")
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        host.database()
            .model_context(&session)
            .unwrap()
            .messages
            .len(),
        4
    );
}

#[test]
fn registered_tool_only_runners_share_preview_delivery_and_save_only_recovery() {
    for fail in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut runner = Runner::new(fail);
        runner.tool_previews = true;
        let outputs = &mut runner.manifest.definition.outputs;
        outputs.retain(|output| output.name != "answer");
        outputs.push(
            serde_json::from_value(serde_json::json!({
                "name":"tool_call_delta", "node":"remote", "port":"tool_call_delta", "optional":true
            }))
            .unwrap(),
        );
        let runner = Arc::new(runner);
        let profile = WorkflowProfile::with_backend("runner", runner.clone()).unwrap();
        let host =
            ChatHost::open(root.path().join("chat.sqlite"), HostConfig::new(profile)).unwrap();
        let session = host
            .database()
            .create_session("runner", None, None)
            .unwrap();
        let (job, receiver) = host.submit_stream(&session.id, "canonical").unwrap();
        let invocation = job.invocation().clone();
        let mut fragments = Vec::new();
        while let Ok(event) = receiver.recv() {
            assert_eq!(event.invocation, invocation);
            let mf_chat::PreviewDelta::ToolCall { delta } = event.delta else {
                panic!("unexpected text")
            };
            assert_eq!(delta.index, 0);
            fragments.push(delta.arguments);
        }
        // A backend failure may cancel queued previews before delivery, but not duplicate them.
        assert!(["{", "}"].starts_with(&fragments.iter().map(String::as_str).collect::<Vec<_>>()));
        if !fail {
            assert_eq!(fragments, ["{", "}"]);
        }
        let completion = if fail {
            let ChatError::Uncommitted { pending, .. } = job.join().unwrap_err() else {
                panic!("expected retained result")
            };
            assert_eq!(
                host.database().model_context(&session.id).unwrap().revision,
                0
            );
            host.database().save_recovered(*pending).unwrap()
        } else {
            job.join().unwrap()
        };
        assert_eq!(completion.result.final_text(), Some("canonical"));
        assert_eq!(
            host.database()
                .messages(&session.id, 50, None)
                .unwrap()
                .messages
                .len(),
            2
        );
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
        if !fail {
            host.submit(&session.id, "no subscriber")
                .unwrap()
                .join()
                .unwrap();
        }
        host.close().unwrap();
    }
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
    assert_eq!(pending.completion.result.final_text(), Some("retained"));
    host.database().save_recovered(*pending).unwrap();
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        host.database()
            .model_context(&session)
            .unwrap()
            .messages
            .len(),
        2
    );
}
