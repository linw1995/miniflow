//! Application workflow hosting over the shared conversation protocol.

use crate::{
    ChatDatabase, ChatError, Completion, ConversationInputs, ConversationResult, Invocation, Turn,
    contract::{ChatContract, CompletionState, ReconciledCompletion, TurnExit},
    error::*,
    flows,
    nodes::{BeginTurnInputs, CompleteTurnInputs, TurnStarted},
};
use mf_compiler::{CompiledWorkflow, FlowBuilder, WorkflowDefinition};
use mf_runtime::{
    ExecutionContext, FlowNode, FlowRuntime, NodeMetadata, NodeRegistry, NodeValue, PreparedNode,
    StreamCancellation, TypedNodeResult, TypedTaskNode, WorkflowArguments,
};
use mf_telemetry::{identity::RunId, observation::Observer};
use snafu::{IntoError, ResultExt, ensure};
use std::sync::{Arc, Mutex};

/// Supplies one final protocol result. Transport and model composition belong to the backend.
pub trait ConversationBackend: Send + Sync {
    fn invoke(
        &self,
        invocation: &Invocation,
        request: ConversationInputs,
        context: &mut ExecutionContext,
        observer: Option<&Observer>,
    ) -> Result<ConversationResult, ChatError>;
}

#[derive(Clone)]
pub struct WorkflowProfile {
    pub name: String,
    backend: Arc<dyn ConversationBackend>,
}

impl WorkflowProfile {
    pub fn new(
        name: impl Into<String>,
        definition: WorkflowDefinition,
        parameters: WorkflowArguments,
    ) -> Result<Self, ChatError> {
        let registry = NodeRegistry::from_inventory()?;
        let plan = mf_compiler::compile_definition(&definition, &registry)?;
        let contract = ChatContract::from_manifest(&plan.manifest(&registry)?)?;
        let parameters =
            mf_runtime::resolve_workflow_inputs(&contract.parameters, &parameters, |name| {
                std::env::var_os(name)
            })?;
        Self::with_backend(
            name,
            Arc::new(EmbeddedBackend {
                plan,
                contract,
                parameters,
            }),
        )
    }

    pub fn with_backend(
        name: impl Into<String>,
        backend: Arc<dyn ConversationBackend>,
    ) -> Result<Self, ChatError> {
        let name = name.into();
        ensure!(
            !name.trim().is_empty(),
            InvalidSnafu {
                field: "profile",
                reason: "must be nonblank"
            }
        );
        Ok(Self { name, backend })
    }

    pub fn backend(&self) -> Arc<dyn ConversationBackend> {
        Arc::clone(&self.backend)
    }
}

struct EmbeddedBackend {
    plan: CompiledWorkflow,
    contract: ChatContract,
    parameters: WorkflowArguments,
}

impl ConversationBackend for EmbeddedBackend {
    fn invoke(
        &self,
        _: &Invocation,
        request: ConversationInputs,
        context: &mut ExecutionContext,
        observer: Option<&Observer>,
    ) -> Result<ConversationResult, ChatError> {
        let observation = observer
            .map(|observer| self.plan.start_observation(observer, RunId::new()))
            .transpose()?;
        // Application and user graphs may reuse node IDs; expose only protocol inputs.
        let mut child =
            context.fork_domain_snapshot(observation, mf_runtime::OutputVisibility::Borrowed(&[]));
        let _span = child.observation().map(mf_runtime::RunObservation::enter);
        let result = (|| -> Result<_, ChatError> {
            child.set_workflow_arguments(self.contract.arguments(request, &self.parameters)?);
            let registry = NodeRegistry::from_inventory()?;
            let flow = mf_compiler::instantiate_compiled_with_observation(
                &self.plan,
                &registry,
                child.observation_mut(),
            )?;
            ConversationResult::decode(
                FlowRuntime::default().execute_in_context(&flow, &mut child)?,
            )
        })();
        if let Some(run) = child.observation_mut() {
            run.finish(
                result
                    .as_ref()
                    .err()
                    .map(|error| error as &dyn std::fmt::Display),
            );
        }
        result
    }
}

#[derive(Clone, Debug, NodeValue)]
pub struct Settlement {
    pub completion: Option<Completion>,
    pub turn: Option<Turn>,
}

#[derive(Clone, Debug, NodeValue)]
pub struct SettlementOutputs {
    pub settlement: Settlement,
}

fn settle(input: CompleteTurnInputs) -> Result<SettlementOutputs, ChatError> {
    let settlement = settle_result(input)?;
    Ok(SettlementOutputs { settlement })
}

fn settle_result(input: CompleteTurnInputs) -> Result<Settlement, ChatError> {
    input.response.validate()?;
    if input.response.status == "succeeded" {
        Ok(Settlement {
            completion: Some(crate::turn::complete(input)?.completion),
            turn: None,
        })
    } else {
        crate::turn::fail(crate::nodes::FailTurnInputs {
            database_path: input.database_path.clone(),
            invocation: input.invocation.clone(),
            state: "failed".into(),
            error: input
                .response
                .error
                .expect("validated failure has an error"),
            usages: Some(input.response.usages),
        })?;
        Ok(Settlement {
            completion: None,
            turn: Some(crate::turn::load_turn(
                &mut crate::storage::connect(std::path::Path::new(&input.database_path))?,
                &input.invocation.turn_id,
            )?),
        })
    }
}

register_operation!(
    "mf_chat.turn.settle",
    Settle,
    CompleteTurnInputs,
    SettlementOutputs,
    settle
);

struct UnboundWorkflow;

impl TypedTaskNode for UnboundWorkflow {
    type Input = TurnStarted;
    type Output = CompleteTurnInputs;

    fn execute(
        &self,
        _: Self::Input,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
        Err(InvalidSnafu {
            field: "conversation backend",
            reason: "must be bound by the application host",
        }
        .build()
        .into())
    }
}

register_task!("mf_chat.workflow.invoke", UnboundWorkflow);

struct InvokeWorkflow {
    backend: Arc<dyn ConversationBackend>,
    automatic_titles: bool,
    observer: Option<Observer>,
    delivered: Arc<Mutex<Option<ConversationResult>>>,
}

impl TypedTaskNode for InvokeWorkflow {
    type Input = TurnStarted;
    type Output = CompleteTurnInputs;

    fn execute(
        &self,
        mut input: Self::Input,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
        if let Some(context) = &mut input.request.context {
            context.title.enabled = self.automatic_titles;
        }
        let response = self.backend.invoke(
            &input.invocation,
            input.request,
            context,
            self.observer.as_ref(),
        )?;
        response.validate()?;
        // Retain only the backend's final protocol result for save-only recovery.
        *self
            .delivered
            .lock()
            .expect("delivery state was not poisoned") = Some(response.clone());
        Ok(CompleteTurnInputs {
            database_path: input.database_path,
            invocation: input.invocation,
            response,
        }
        .into())
    }
}

/// Executes the application-owned admission, invocation, and persistence graph.
pub fn run_conversation(
    database: &ChatDatabase,
    request: BeginTurnInputs,
    backend: Arc<dyn ConversationBackend>,
    cancellation: StreamCancellation,
    automatic_titles: bool,
    observer: Option<&Observer>,
) -> Result<ReconciledCompletion, ChatError> {
    ensure!(
        std::path::Path::new(&request.database_path) == database.path(),
        InvalidSnafu {
            field: "database_path",
            reason: "must identify the owning host database"
        }
    );
    let invocation = Invocation {
        session_id: request.session_id.clone(),
        turn_id: request.turn_id.clone(),
        call_id: request.call_id.clone(),
    };
    let plan = flows::conversation::plan()?;
    let registry = NodeRegistry::from_inventory()?;
    let delivered = Arc::new(Mutex::new(None));
    let mut nodes = mf_compiler::resolve_nodes(&plan.definition, &registry)?;
    let order = plan.execution_order.clone();
    let bound = PreparedNode::typed_task(
        InvokeWorkflow {
            backend,
            automatic_titles,
            observer: observer.cloned(),
            delivered: Arc::clone(&delivered),
        },
        NodeMetadata::default(),
    )
    .context(BackendBuildSnafu)?;
    let node = nodes
        .iter_mut()
        .find(|node| node.definition_id.as_str() == "workflow")
        .expect("embedded conversation declares the invocation node");
    *node = FlowNode::new(node.definition_id.clone(), bound);
    let flow = FlowBuilder::prepare(
        nodes,
        plan.definition.edges.clone(),
        order,
        plan.definition.outputs.clone(),
    )?
    .with_control_edges(plan.definition.control_edges.clone())?
    .with_order_edges(plan.definition.order_edges.clone())?
    .with_workflow_inputs()?
    .into_tasks()?;
    let mut arguments = WorkflowArguments::default();
    for (name, value) in request.into_values()? {
        arguments
            .0
            .insert(mf_runtime::output_id("begin", &name), value);
    }
    let observation = observer
        .map(|observer| plan.start_observation(observer, RunId::new()))
        .transpose()?;
    let result = ExecutionContext::run(observation, |context| -> Result<_, ChatError> {
        context.set_cancellation(cancellation.clone());
        context.set_workflow_arguments(arguments);
        Ok(FlowRuntime::default().execute_in_context(&flow, context)?)
    });
    match result {
        Ok(outputs) => {
            let settlement = SettlementOutputs::from_values(outputs)?.settlement;
            let state = if let Some(completion) = settlement.completion {
                CompletionState::Committed(completion)
            } else {
                CompletionState::Finished(
                    settlement
                        .turn
                        .expect("settlement returns a terminal state"),
                )
            };
            Ok(ReconciledCompletion {
                state,
                diagnostics: vec![],
            })
        }
        Err(source) => {
            let mut outcome = if cancellation.failure().is_some() {
                TurnExit::Cancelled
            } else {
                TurnExit::Failed
            };
            let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&source);
            while let Some(error) = cause {
                if let Some(ChatError::Backend { outcome: exit, .. }) =
                    error.downcast_ref::<ChatError>()
                {
                    outcome = *exit;
                    break;
                }
                cause = error.source();
            }
            let response = delivered
                .lock()
                .expect("delivery state was not poisoned")
                .take();
            match database.reconcile_result(
                &invocation,
                response.map(Ok),
                outcome,
                &source.to_string(),
            ) {
                Ok(resolution) if matches!(resolution.state, CompletionState::NotBegun) => {
                    Err(source)
                }
                Ok(mut resolution) => {
                    resolution.diagnostics.push(source);
                    Ok(resolution)
                }
                Err(cleanup) => Err(ExecutionSnafu {
                    invocation,
                    cleanup: Some(Box::new(cleanup)),
                }
                .into_error(Box::new(source))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ReadContext;

    impl TypedTaskNode for ReadContext {
        type Input = ConversationInputs;
        type Output = ConversationResult;

        fn execute(
            &self,
            _: Self::Input,
            context: &mut ExecutionContext,
        ) -> Result<TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
            Ok(ConversationResult {
                text: if context.output("begin.database_path").is_ok() {
                    "leaked"
                } else {
                    "isolated"
                }
                .into(),
                ..Default::default()
            }
            .into())
        }
    }

    register_task!("test.chat.context", ReadContext);

    #[test]
    fn user_workflows_cannot_read_application_context_outputs() {
        let source = FlowNode::new(
            "begin",
            PreparedNode::typed_task(crate::nodes::Initialize, NodeMetadata::default()).unwrap(),
        );
        let mut context = ExecutionContext::for_message(
            &source,
            mf_runtime::Outputs::from([("database_path".into(), "private.sqlite".into())]).into(),
        )
        .unwrap();
        let definition = WorkflowDefinition::from_json(
            r#"{
            "version":"2026-10-19", "dependencies":{},
            "nodes":[{"id":"backend","kind":"test.chat.context"}],
            "outputs":[
                {"name":"status","node":"backend","port":"status"},
                {"name":"text","node":"backend","port":"text"},
                {"name":"usages","node":"backend","port":"usages"}
            ]
        }"#,
        )
        .unwrap();
        let profile = WorkflowProfile::new("custom", definition, Default::default()).unwrap();
        let result = profile
            .backend
            .invoke(
                &Invocation::new("session"),
                ConversationInputs {
                    prompt: "hello".into(),
                    system: None,
                    history: None,
                    context: None,
                },
                &mut context,
                None,
            )
            .unwrap();
        assert_eq!(result.text, "isolated");
        assert!(context.output("begin.database_path").is_ok());
    }

    struct Backend(Arc<AtomicUsize>);

    impl ConversationBackend for Backend {
        fn invoke(
            &self,
            _: &Invocation,
            input: ConversationInputs,
            _: &mut ExecutionContext,
            _: Option<&Observer>,
        ) -> Result<ConversationResult, ChatError> {
            assert!(input.context.is_some());
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(ConversationResult {
                text: "answer".into(),
                ..Default::default()
            })
        }
    }

    #[test]
    fn bound_backends_use_application_admission_without_model_knowledge() {
        let directory = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let profile =
            WorkflowProfile::with_backend("custom", Arc::new(Backend(Arc::clone(&calls)))).unwrap();
        let mut config = crate::HostConfig::new(profile);
        config.max_history_bytes = 32;
        let host = crate::ChatHost::open(directory.path().join("chat.sqlite"), config).unwrap();
        let session = host
            .database()
            .create_session("custom", None, None)
            .unwrap();
        host.submit(&session.id, "first").unwrap().join().unwrap();
        let error = host
            .submit(&session.id, "second")
            .unwrap()
            .join()
            .unwrap_err();
        assert!(error.to_string().contains("history exceeds"));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            host.database()
                .messages(&session.id, 10, None)
                .unwrap()
                .messages
                .len(),
            2
        );
        host.close().unwrap();
    }

    #[test]
    fn invocation_requires_an_explicit_backend_binding() {
        let input = TurnStarted {
            database_path: "unused.sqlite".into(),
            invocation: Invocation::new("session"),
            request: ConversationInputs {
                prompt: "hello".into(),
                system: None,
                history: None,
                context: None,
            },
        };
        let error = UnboundWorkflow
            .execute(input, &mut ExecutionContext::default())
            .err()
            .unwrap();
        assert!(error.to_string().contains("must be bound"));
    }

    #[test]
    fn application_cannot_write_through_another_database_path() {
        let directory = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
        let request = BeginTurnInputs {
            database_path: directory
                .path()
                .join("other.sqlite")
                .to_str()
                .unwrap()
                .into(),
            session_id: "session".into(),
            turn_id: "turn".into(),
            call_id: "call".into(),
            prompt: "hello".into(),
            model: crate::ModelIdentity {
                profile: "custom".into(),
                model: None,
            },
            retry_of: None,
            max_history_bytes: crate::DEFAULT_HISTORY_BYTES,
        };
        let result = run_conversation(
            &database,
            request,
            Arc::new(Backend(Arc::default())),
            Default::default(),
            true,
            None,
        );
        assert!(matches!(
            result,
            Err(ChatError::Invalid {
                field: "database_path",
                ..
            })
        ));
        assert!(!directory.path().join("other.sqlite").exists());
    }
}
