//! Application workflow hosting over the shared conversation protocol.

use crate::{
    ChatDatabase, ChatError, Completion, ConversationInputs, ConversationResult, Invocation,
    PreviewConsumer, PreviewEvent, Turn,
    contract::{ChatContract, CompletionState, ReconciledCompletion, TurnExit},
    error::*,
    flows,
    nodes::{BeginTurnInputs, CompleteTurnInputs, TurnStarted},
};
use mf_compiler::{CompiledWorkflow, FlowBuilder, WorkflowDefinition};
use mf_runtime::{
    ExecutionContext, FlowNode, FlowRuntime, NodeMetadata, NodeRegistry, NodeValue, PreparedNode,
    StreamCancellation, TypedNodeResult, TypedStreamNode, WorkflowArguments,
};
use mf_telemetry::{identity::RunId, observation::Observer};
use snafu::{IntoError, OptionExt, ResultExt, ensure};
use std::{
    sync::{Arc, Mutex},
    task::{Wake, Waker},
};

struct CancelWorkflow(StreamCancellation);

impl Wake for CancelWorkflow {
    fn wake(self: Arc<Self>) {
        self.0.cancel(mf_runtime::StreamError::Execution {
            message: "conversation cancelled by host".into(),
        });
    }
}

/// Invokes one backend and forwards optional text deltas into the conversation Flow.
pub trait ConversationBackend: Send + Sync {
    /// Resolved inputs for the optional built-in title Flow, with its `input` namespace.
    fn builtin_title_parameters(&self) -> Option<WorkflowArguments> {
        None
    }

    fn invoke(
        &self,
        invocation: &Invocation,
        request: ConversationInputs,
        context: &mut ExecutionContext,
        preview: Option<&mut dyn FnMut(crate::TextDelta) -> Result<(), ChatError>>,
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
        Self::from_plan(name, plan, &registry, parameters)
    }

    pub fn from_path(
        name: impl Into<String>,
        path: impl AsRef<std::path::Path>,
        parameters: WorkflowArguments,
    ) -> Result<Self, ChatError> {
        let definition = mf_compiler::load_embedded_workflow(path.as_ref())?.definition;
        let registry = NodeRegistry::from_inventory()?;
        let plan = mf_compiler::compile_resolved_definition(&definition, &registry)?;
        Self::from_plan(name, plan, &registry, parameters)
    }

    fn from_plan(
        name: impl Into<String>,
        plan: CompiledWorkflow,
        registry: &NodeRegistry,
        parameters: WorkflowArguments,
    ) -> Result<Self, ChatError> {
        let contract = ChatContract::from_manifest(&plan.manifest(registry)?)?;
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
    fn builtin_title_parameters(&self) -> Option<WorkflowArguments> {
        self.contract.builtin_title_parameters(&self.parameters)
    }

    fn invoke(
        &self,
        _: &Invocation,
        request: ConversationInputs,
        context: &mut ExecutionContext,
        mut preview: Option<&mut dyn FnMut(crate::TextDelta) -> Result<(), ChatError>>,
        observer: Option<&Observer>,
    ) -> Result<ConversationResult, ChatError> {
        let arguments = self.contract.arguments(request, &self.parameters)?;
        let registry = NodeRegistry::from_inventory()?;
        if self.contract.streaming {
            let observation = observer
                .map(|observer| self.plan.start_stream_observation(observer, RunId::new()))
                .transpose()?;
            let prepared = mf_compiler::instantiate_stream(&self.plan, &registry)?;
            if let Some(error) = context.cancellation().failure() {
                return Err(error.into());
            }
            let instance = prepared.start_with_options(mf_runtime::StreamOptions {
                arguments,
                observation,
                ..Default::default()
            })?;
            context
                .cancellation()
                .register(Waker::from(Arc::new(CancelWorkflow(
                    instance.cancellation(),
                ))));
            let mut decoder = crate::contract::ConversationOutputDecoder::new(&self.contract);
            let result = (|| -> Result<(), ChatError> {
                while let Some(delivery) = instance.receive()? {
                    if let Some(delta) = decoder.accept(delivery.output().outputs.clone())?
                        && let Some(consume) = preview.as_mut()
                    {
                        consume(delta)?;
                    }
                    delivery.acknowledge()?;
                }
                instance.join()?;
                Ok(())
            })();
            result.boxed().context(BackendSnafu {
                outcome: TurnExit::Failed,
                reply: decoder.result().cloned().map(Box::new),
            })?;
            decoder.finish()
        } else {
            let observation = observer
                .map(|observer| self.plan.start_observation(observer, RunId::new()))
                .transpose()?;
            // Keep application admission and persistence outside the user graph.
            let mut child = context
                .fork_domain_snapshot(observation, mf_runtime::OutputVisibility::Borrowed(&[]));
            let _span = child.observation().map(mf_runtime::RunObservation::enter);
            let result = (|| -> Result<_, ChatError> {
                child.set_workflow_arguments(arguments);
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

#[derive(Clone, Debug, NodeValue)]
struct TitleRequest {
    prompt: String,
    context: crate::contract::ConversationContext,
    model: mf_runtime::ValueRef,
    base_url: mf_runtime::ValueRef,
    api_key: mf_runtime::ValueRef,
}

impl TitleRequest {
    fn from_inputs(
        request: &ConversationInputs,
        parameters: Option<&WorkflowArguments>,
    ) -> Result<Option<Self>, ChatError> {
        let (Some(parameters), Some(context)) = (parameters, request.context.as_ref()) else {
            return Ok(None);
        };
        let parameter = |name| {
            parameters
                .0
                .get(&mf_runtime::output_id("input", name))
                .cloned()
                .context(InvalidSnafu {
                    field: "title parameters",
                    reason: "requires resolved model, base_url, and api_key inputs",
                })
        };
        Ok(Some(Self {
            prompt: request.prompt.clone(),
            context: context.clone(),
            model: parameter("model")?,
            base_url: parameter("base_url")?,
            api_key: parameter("api_key")?,
        }))
    }
}

#[derive(Clone, Debug, NodeValue)]
struct InvocationEvent {
    preview: Option<PreviewEvent>,
    response: Option<CompleteTurnInputs>,
    title_request: Option<TitleRequest>,
}

#[derive(Clone, Debug, NodeValue)]
struct InvocationEmission {
    event: InvocationEvent,
}

struct UnboundWorkflow;

impl TypedStreamNode for UnboundWorkflow {
    type Input = TurnStarted;
    type Output = InvocationEmission;

    fn execute(
        &mut self,
        _: Self::Input,
        _: &mut ExecutionContext,
        _: &mut dyn FnMut(
            TypedNodeResult<Self::Output>,
        ) -> Result<(), mf_runtime::NodeExecutionError>,
    ) -> Result<(), mf_runtime::NodeExecutionError> {
        Err(InvalidSnafu {
            field: "conversation backend",
            reason: "must be bound by the application host",
        }
        .build()
        .into())
    }
}

inventory::submit! {
    mf_runtime::NodeRegistration {
        kind: "mf_chat.workflow.invoke",
        factory: mf_runtime::NodeFactory::Plain(|_| {
            PreparedNode::typed_stream(UnboundWorkflow, NodeMetadata::default())
        }),
    }
}

struct InvokeWorkflow {
    backend: Arc<dyn ConversationBackend>,
    automatic_titles: bool,
    observer: Observer,
    calls: Arc<Mutex<crate::CallEventCollector>>,
    delivered: Arc<Mutex<Option<ConversationResult>>>,
    previews: bool,
    title_parameters: Option<WorkflowArguments>,
}

impl TypedStreamNode for InvokeWorkflow {
    type Input = TurnStarted;
    type Output = InvocationEmission;

    fn execute(
        &mut self,
        mut input: Self::Input,
        context: &mut ExecutionContext,
        emit: &mut dyn FnMut(
            TypedNodeResult<Self::Output>,
        ) -> Result<(), mf_runtime::NodeExecutionError>,
    ) -> Result<(), mf_runtime::NodeExecutionError> {
        if let Some(context) = &mut input.request.context {
            context.title.enabled = self.automatic_titles;
        }
        let title_request =
            TitleRequest::from_inputs(&input.request, self.title_parameters.as_ref())?;
        let calls = &self.calls;
        let response = {
            let invocation = input.invocation.clone();
            let mut consume = |delta| -> Result<(), ChatError> {
                emit(
                    InvocationEmission {
                        event: InvocationEvent {
                            preview: Some(PreviewEvent {
                                invocation: invocation.clone(),
                                delta,
                            }),
                            response: None,
                            title_request: None,
                        },
                    }
                    .into(),
                )?;
                Ok(())
            };
            let preview = self.previews.then_some(
                &mut consume as &mut dyn FnMut(crate::TextDelta) -> Result<(), ChatError>,
            );
            self.backend.invoke(
                &input.invocation,
                input.request,
                context,
                preview,
                Some(&self.observer),
            )
        };
        if let Err(error) = &response {
            let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error);
            while let Some(error) = cause {
                if let Some(ChatError::Backend {
                    reply: Some(reply), ..
                }) = error.downcast_ref::<ChatError>()
                {
                    let mut reply = (**reply).clone();
                    calls.lock().unwrap().apply(&mut reply.usages);
                    *self.delivered.lock().unwrap() = Some(reply);
                    break;
                }
                cause = error.source();
            }
        }
        let mut response = response?;
        calls.lock().unwrap().apply(&mut response.usages);
        response.validate()?;
        if self.title_parameters.is_none() {
            *self
                .delivered
                .lock()
                .expect("delivery state was not poisoned") = Some(response.clone());
        }
        emit(
            InvocationEmission {
                event: InvocationEvent {
                    preview: None,
                    response: Some(CompleteTurnInputs {
                        database_path: input.database_path,
                        invocation: input.invocation,
                        response,
                    }),
                    title_request,
                },
            }
            .into(),
        )
    }
}

#[derive(NodeValue)]
struct TitleResult {
    title: Option<String>,
    usages: Vec<crate::CallUsage>,
}

#[derive(NodeValue)]
struct CompleteConversationInputs {
    response: ConversationResult,
    title_result: TitleResult,
}

#[derive(NodeValue)]
struct CompleteConversationOutputs {
    response: ConversationResult,
}

fn complete_conversation(
    mut input: CompleteConversationInputs,
) -> Result<CompleteConversationOutputs, ChatError> {
    if let Some(title) = input.title_result.title {
        input.response.title = Some(title);
    }
    input.response.usages.extend(input.title_result.usages);
    input.response.validate()?;
    Ok(CompleteConversationOutputs {
        response: input.response,
    })
}

register_operation!(
    "mf_chat.conversation.complete",
    CompleteConversation,
    CompleteConversationInputs,
    CompleteConversationOutputs,
    complete_conversation
);

struct CaptureCompletion {
    calls: Arc<Mutex<crate::CallEventCollector>>,
    delivered: Arc<Mutex<Option<ConversationResult>>>,
}

impl mf_runtime::TypedTaskNode for CaptureCompletion {
    type Input = CompleteConversationInputs;
    type Output = CompleteConversationOutputs;

    fn execute(
        &self,
        input: Self::Input,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
        let mut output = complete_conversation(input)?;
        self.calls
            .lock()
            .expect("call events were not poisoned")
            .apply(&mut output.response.usages);
        *self
            .delivered
            .lock()
            .expect("delivery state was not poisoned") = Some(output.response.clone());
        Ok(output.into())
    }
}

#[derive(Debug, Default, NodeValue)]
struct RoutedEvent {
    preview: Option<PreviewEvent>,
    database_path: Option<String>,
    invocation: Option<Invocation>,
    response: Option<ConversationResult>,
    title_enabled: Option<bool>,
    title_text: Option<String>,
    title_status: Option<String>,
    title_prompt: Option<String>,
    title_context: Option<crate::contract::ConversationContext>,
    title_model: Option<mf_runtime::ValueRef>,
    title_base_url: Option<mf_runtime::ValueRef>,
    title_api_key: Option<mf_runtime::ValueRef>,
}

#[derive(Default)]
struct RouteInvocation {
    response: Option<CompleteTurnInputs>,
    title_request: Option<TitleRequest>,
}

impl mf_runtime::TypedEventNode for RouteInvocation {
    type Input = InvocationEmission;
    type Output = RoutedEvent;

    fn on_event(
        &mut self,
        event: mf_runtime::NodeEvent<Self::Input>,
        _: &mf_runtime::EventContext<'_>,
    ) -> Result<mf_runtime::EventEffects<Self::Output>, mf_runtime::NodeExecutionError> {
        let (outputs, skipped) = match event {
            mf_runtime::NodeEvent::Input(input) => {
                ensure!(
                    self.response.is_none(),
                    InvalidSnafu {
                        field: "conversation event",
                        reason: "cannot follow the final response"
                    }
                );
                ensure!(
                    input.event.preview.is_some() != input.event.response.is_some(),
                    InvalidSnafu {
                        field: "conversation event",
                        reason: "requires exactly one preview or final response"
                    }
                );
                if let Some(preview) = input.event.preview {
                    preview.validate()?;
                    (
                        RoutedEvent {
                            preview: Some(preview),
                            ..Default::default()
                        },
                        vec![
                            "database_path".into(),
                            "invocation".into(),
                            "response".into(),
                            "title_enabled".into(),
                            "title_text".into(),
                            "title_status".into(),
                            "title_prompt".into(),
                            "title_context".into(),
                            "title_model".into(),
                            "title_base_url".into(),
                            "title_api_key".into(),
                        ],
                    )
                } else {
                    self.response = input.event.response;
                    self.title_request = input.event.title_request;
                    return Ok(Default::default());
                }
            }
            mf_runtime::NodeEvent::UpstreamClosed => {
                let response = self.response.take().context(InvalidSnafu {
                    field: "conversation event",
                    reason: "ended without a final response",
                })?;
                let title_enabled = self.title_request.is_some()
                    && response.response.status == "succeeded"
                    && response.response.title.is_none()
                    && !response.response.usages.iter().any(|usage| {
                        usage
                            .attribution
                            .as_ref()
                            .and_then(|attribution| attribution.purpose.as_deref())
                            == Some("title")
                    });
                let mut output = RoutedEvent {
                    title_text: Some(response.response.text.clone()),
                    title_status: Some(response.response.status.clone()),
                    database_path: Some(response.database_path),
                    invocation: Some(response.invocation),
                    response: Some(response.response),
                    title_enabled: Some(title_enabled),
                    ..Default::default()
                };
                let mut skipped = vec!["preview".into()];
                if let Some(request) = self.title_request.take() {
                    output.title_prompt = Some(request.prompt);
                    output.title_context = Some(request.context);
                    output.title_model = Some(request.model);
                    output.title_base_url = Some(request.base_url);
                    output.title_api_key = Some(request.api_key);
                } else {
                    skipped.extend(
                        [
                            "title_prompt",
                            "title_context",
                            "title_model",
                            "title_base_url",
                            "title_api_key",
                        ]
                        .map(String::from),
                    );
                }
                (output, skipped)
            }
            mf_runtime::NodeEvent::Timer => return Ok(Default::default()),
        };
        Ok(mf_runtime::EventEffects {
            emissions: vec![
                TypedNodeResult {
                    outputs,
                    skipped: skipped.into_iter().collect(),
                    loop_summary: None,
                }
                .into(),
            ],
            timer: Default::default(),
        })
    }

    fn buffered_items(&self) -> Option<usize> {
        Some(usize::from(self.response.is_some()))
    }
}

inventory::submit! {
    mf_runtime::NodeRegistration {
        kind: "mf_chat.conversation.route",
        factory: mf_runtime::NodeFactory::Plain(|_| {
            PreparedNode::typed_event(RouteInvocation::default(), NodeMetadata::default())
        }),
    }
}

#[derive(NodeValue)]
struct PreviewOutput {
    preview: PreviewEvent,
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
    run_conversation_with_preview(
        database,
        request,
        backend,
        cancellation,
        automatic_titles,
        observer,
        None,
    )
}

pub fn run_conversation_with_preview(
    database: &ChatDatabase,
    request: BeginTurnInputs,
    backend: Arc<dyn ConversationBackend>,
    cancellation: StreamCancellation,
    automatic_titles: bool,
    observer: Option<&Observer>,
    preview: Option<PreviewConsumer>,
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
    let calls = Arc::new(Mutex::new(crate::CallEventCollector::default()));
    let collected = calls.clone();
    let _events = mf_telemetry::events::collect(move |event| {
        let _ = collected.lock().unwrap().observe(event);
    });
    let title_parameters = automatic_titles
        .then(|| backend.builtin_title_parameters())
        .flatten();
    let observer = observer.cloned().unwrap_or_else(|| Observer::stderr(0));
    let mut plan = flows::conversation::plan()?;
    let registry = NodeRegistry::from_inventory()?;
    let delivered = Arc::new(Mutex::new(None));
    let mut nodes = mf_compiler::resolve_nodes(&plan.definition, &registry)?;
    let order = plan.execution_order.clone();
    let bound = PreparedNode::typed_stream(
        InvokeWorkflow {
            backend,
            automatic_titles,
            observer: observer.clone(),
            calls: Arc::clone(&calls),
            delivered: Arc::clone(&delivered),
            previews: preview.is_some(),
            title_parameters,
        },
        NodeMetadata::default(),
    )
    .context(BackendBuildSnafu)?;
    let node = nodes
        .iter_mut()
        .find(|node| node.definition_id.as_str() == "workflow")
        .expect("embedded conversation declares the invocation node");
    *node = FlowNode::new(node.definition_id.clone(), bound);
    let completion = nodes
        .iter_mut()
        .find(|node| node.definition_id.as_str() == "complete")
        .expect("application conversation declares completion");
    *completion = FlowNode::new(
        completion.definition_id.clone(),
        PreparedNode::typed_task(
            CaptureCompletion {
                calls,
                delivered: Arc::clone(&delivered),
            },
            NodeMetadata::default(),
        )
        .context(BackendBuildSnafu)?,
    );
    plan.definition = mf_compiler::resolve_execution(&plan.definition, &nodes)?;
    let flow = FlowBuilder::prepare(
        nodes,
        plan.definition.edges.clone(),
        order,
        plan.definition.outputs.clone(),
    )?
    .with_control_edges(plan.definition.control_edges.clone())?
    .with_order_edges(plan.definition.order_edges.clone())?
    .with_workflow_inputs()?
    .into_stream(
        plan.definition
            .execution
            .clone()
            .expect("conversation is a Stream Flow"),
    )?;
    let mut arguments = WorkflowArguments::default();
    for (name, value) in request.into_values()? {
        arguments
            .0
            .insert(mf_runtime::output_id("begin", &name), value);
    }
    let observation = Some(plan.start_stream_observation(&observer, RunId::new())?);
    let result = (|| -> Result<Settlement, ChatError> {
        if let Some(error) = cancellation.failure() {
            return Err(error.into());
        }
        let instance = flow.start_with_options(mf_runtime::StreamOptions {
            arguments,
            observation,
            ..Default::default()
        })?;
        let runtime_cancellation = instance.cancellation();
        cancellation.register(Waker::from(Arc::new(CancelWorkflow(
            runtime_cancellation.clone(),
        ))));
        let mut settlement = None;
        while let Some(delivery) = instance.receive()? {
            let mut outputs = delivery.output().outputs.clone();
            if let Some(event) = outputs.remove("preview") {
                let event = PreviewOutput::from_values([("preview".into(), event)].into())?.preview;
                if let Some(consumer) = &preview {
                    consumer.send(event, delivery, &runtime_cancellation)?;
                    continue;
                }
            }
            if outputs.contains_key("settlement") {
                ensure!(
                    settlement.is_none(),
                    InvalidSnafu {
                        field: "conversation settlement",
                        reason: "must occur exactly once"
                    }
                );
                settlement = Some(SettlementOutputs::from_values(outputs)?.settlement);
            }
            delivery.acknowledge()?;
        }
        instance.join()?;
        settlement.ok_or_else(|| {
            CaptureSnafu {
                message: "conversation ended without settlement",
            }
            .build()
        })
    })();
    match result {
        Ok(settlement) => {
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
                    && cancellation.failure().is_none()
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
    use mf_runtime::TypedTaskNode;
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
            _: Option<&mut dyn FnMut(crate::TextDelta) -> Result<(), ChatError>>,
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

    fn final_event() -> InvocationEmission {
        InvocationEmission {
            event: InvocationEvent {
                title_request: None,
                preview: None,
                response: Some(CompleteTurnInputs {
                    database_path: "chat.sqlite".into(),
                    invocation: Invocation::new("session"),
                    response: ConversationResult {
                        text: "canonical".into(),
                        ..Default::default()
                    },
                }),
            },
        }
    }

    #[test]
    fn conversation_route_waits_for_normal_close_before_releasing_settlement() {
        use mf_runtime::{EventContext, NodeEvent, TypedEventNode};
        let context = EventContext {
            now: Default::default(),
            input: None,
        };
        let mut route = RouteInvocation::default();
        let preview = PreviewEvent {
            invocation: Invocation::new("session"),
            delta: crate::TextDelta {
                text: "prefix".into(),
                model_call_id: "model".into(),
            },
        };
        let effects = route
            .on_event(
                NodeEvent::Input(InvocationEmission {
                    event: InvocationEvent {
                        preview: Some(preview.clone()),
                        response: None,
                        title_request: None,
                    },
                }),
                &context,
            )
            .unwrap();
        assert_eq!(effects.emissions.len(), 1);
        assert_eq!(effects.emissions[0].result.outputs.preview, Some(preview));
        assert!(effects.emissions[0].result.skipped.contains("response"));
        assert!(
            route
                .on_event(NodeEvent::Input(final_event()), &context)
                .unwrap()
                .emissions
                .is_empty()
        );
        assert_eq!(route.buffered_items(), Some(1));
        assert!(
            route
                .on_event(NodeEvent::Timer, &context)
                .unwrap()
                .emissions
                .is_empty()
        );
        let effects = route.on_event(NodeEvent::UpstreamClosed, &context).unwrap();
        assert_eq!(effects.emissions.len(), 1);
        assert_eq!(
            effects.emissions[0]
                .result
                .outputs
                .response
                .as_ref()
                .unwrap()
                .text,
            "canonical"
        );
        assert!(effects.emissions[0].result.skipped.contains("preview"));
        assert_eq!(route.buffered_items(), Some(0));
    }

    #[test]
    fn title_inputs_cross_the_stream_boundary_only_after_close_and_keep_secrets() {
        use mf_runtime::{EventContext, NodeEvent, TypedEventNode};
        let context = EventContext {
            now: Default::default(),
            input: None,
        };
        let mut event = final_event();
        let request = ConversationInputs {
            prompt: "Prompt".into(),
            system: None,
            history: None,
            context: Some(crate::contract::ConversationContext {
                title: crate::contract::TitleContext {
                    source: "provisional".into(),
                    revision: 0,
                    attempted: false,
                    enabled: true,
                },
            }),
        };
        let parameters = WorkflowArguments(
            [
                ("input.model".into(), "model".into()),
                ("input.base_url".into(), "http://localhost/v1".into()),
                (
                    "input.api_key".into(),
                    mf_runtime::ValueRef::from("credential").into_secret(),
                ),
            ]
            .into(),
        );
        event.event.title_request = TitleRequest::from_inputs(&request, Some(&parameters)).unwrap();
        let decoded = InvocationEmission::from_values(event.into_values().unwrap()).unwrap();
        let mut route = RouteInvocation::default();
        assert!(
            route
                .on_event(NodeEvent::Input(decoded), &context)
                .unwrap()
                .emissions
                .is_empty()
        );
        let closed = route.on_event(NodeEvent::UpstreamClosed, &context).unwrap();
        let output = &closed.emissions[0].result.outputs;
        assert_eq!(output.title_enabled, Some(true));
        assert_eq!(output.title_prompt.as_deref(), Some("Prompt"));
        assert_eq!(output.title_text.as_deref(), Some("canonical"));
        let api_key = output.title_api_key.as_ref().unwrap();
        assert_eq!(api_key.as_str(), Some("credential"));
        assert!(api_key.is_secret());
        assert!(!output.title_model.as_ref().unwrap().is_secret());
    }

    #[test]
    fn conversation_route_rejects_missing_final_and_events_after_final() {
        use mf_runtime::{EventContext, NodeEvent, TypedEventNode};
        let context = EventContext {
            now: Default::default(),
            input: None,
        };
        let mut route = RouteInvocation::default();
        assert!(route.on_event(NodeEvent::UpstreamClosed, &context).is_err());
        assert!(
            route
                .on_event(
                    NodeEvent::Input(InvocationEmission {
                        event: InvocationEvent {
                            preview: None,
                            response: None,
                            title_request: None,
                        },
                    }),
                    &context
                )
                .is_err()
        );
        route
            .on_event(NodeEvent::Input(final_event()), &context)
            .unwrap();
        assert!(
            route
                .on_event(NodeEvent::Input(final_event()), &context)
                .is_err()
        );
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
            .execute(input, &mut ExecutionContext::default(), &mut |_| Ok(()))
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
