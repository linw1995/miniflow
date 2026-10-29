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
    ExecutionContext, FlowNode, NodeMetadata, NodeRegistry, NodeValue, PreparedNode,
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
    /// Exposes a finite embedded Flow and resolved parameters for `workflow.call` composition.
    /// Other backends retain the host-bound invocation adapter.
    fn embedded_workflow(&self) -> Option<(&CompiledWorkflow, &WorkflowArguments)> {
        None
    }

    /// Resolved inputs for the optional built-in title Flow, with its `params` namespace.
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
                call: compile_call(&plan.definition, registry)?,
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
    call: Option<CompiledWorkflow>,
    plan: CompiledWorkflow,
    contract: ChatContract,
    parameters: WorkflowArguments,
}

fn compile_call(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<Option<CompiledWorkflow>, ChatError> {
    let (nodes, _) = mf_compiler::prepare_definition(definition, registry)?;
    if nodes.iter().any(|node| {
        matches!(
            node.node,
            Some(mf_runtime::NodeExecution::Event(_) | mf_runtime::NodeExecution::Stream(_))
        )
    }) {
        return Ok(None);
    }
    let streams: std::collections::BTreeSet<_> = nodes
        .iter()
        .flat_map(|node| {
            node.metadata
                .stream_outputs
                .iter()
                .map(move |port| (node.definition_id.as_str(), port.name.as_ref()))
        })
        .collect();
    if definition.outputs.iter().any(|output| {
        output.name == "answer" && !streams.contains(&(output.node.as_str(), output.port.as_ref()))
    }) || definition
        .edges
        .iter()
        .any(|edge| streams.contains(&(edge.from_node.as_str(), edge.from_output.as_ref())))
        || definition
            .control_edges
            .iter()
            .any(|edge| streams.contains(&(edge.from_node.as_str(), edge.from_output.as_ref())))
    {
        return Ok(None);
    }
    let outputs: Vec<_> = definition.outputs.iter()
        .filter(|output| ConversationResult::ports().iter().any(|port| port.name == output.name))
        .map(|output| serde_json::json!({
        "name": output.name, "node": "workflow", "port": output.name, "optional": output.optional
    })).collect();
    let definition = serde_json::from_value(serde_json::json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [{"id": "workflow", "kind": "workflow.call", "config": {
            "flow": "conversation", "definition": definition
        }}],
        "outputs": outputs
    }))
    .context(JsonSnafu)?;
    Ok(Some(mf_compiler::compile_resolved_definition(
        &definition,
        registry,
    )?))
}

struct ConversationSink<'a> {
    decoder: crate::contract::ConversationOutputDecoder,
    preview: Option<&'a mut dyn FnMut(crate::TextDelta) -> Result<(), ChatError>>,
}

impl mf_runtime::EmissionSink for ConversationSink<'_> {
    fn send(&mut self, result: mf_runtime::NodeResult) -> Result<(), mf_runtime::StreamError> {
        let result = (|| -> Result<(), ChatError> {
            if let Some(delta) = self.decoder.accept(result.outputs)?
                && let Some(consume) = &mut self.preview
            {
                consume(delta)?;
            }
            Ok(())
        })();
        result
            .map_err(mf_runtime::NodeExecutionError::from)
            .context(mf_runtime::StreamProducerSnafu {
                definition_id: "workflow",
            })
    }
}

impl ConversationBackend for EmbeddedBackend {
    fn embedded_workflow(&self) -> Option<(&CompiledWorkflow, &WorkflowArguments)> {
        self.call.as_ref().map(|_| (&self.plan, &self.parameters))
    }

    fn builtin_title_parameters(&self) -> Option<WorkflowArguments> {
        self.contract.builtin_title_parameters(&self.parameters)
    }

    fn invoke(
        &self,
        _: &Invocation,
        request: ConversationInputs,
        context: &mut ExecutionContext,
        preview: Option<&mut dyn FnMut(crate::TextDelta) -> Result<(), ChatError>>,
        observer: Option<&Observer>,
    ) -> Result<ConversationResult, ChatError> {
        if let Some(error) = context.cancellation().failure() {
            return Err(error.into());
        }
        let Some(call) = &self.call else {
            return self.invoke_stream(request, context, preview, observer);
        };
        let arguments = self.contract.arguments(request, &self.parameters)?;
        let arguments = WorkflowArguments(
            arguments
                .0
                .into_iter()
                .map(|(name, value)| (mf_runtime::output_id("workflow", &name), value))
                .collect(),
        );
        let registry = NodeRegistry::from_inventory()?;
        let observation = observer
            .map(|observer| call.start_observation(observer, RunId::new()))
            .transpose()?;
        let mut child =
            context.fork_domain_snapshot(observation, mf_runtime::OutputVisibility::Borrowed(&[]));
        let _span = child.observation().map(mf_runtime::RunObservation::enter);
        let mut sink = ConversationSink {
            decoder: crate::contract::ConversationOutputDecoder::new(&self.contract),
            preview,
        };
        let result = (|| -> Result<_, ChatError> {
            child.set_workflow_arguments(arguments);
            let flow = mf_compiler::instantiate_compiled_with_observation(
                call,
                &registry,
                child.observation_mut(),
            )?;
            let streams = flow.nodes()[0]
                .metadata
                .stream_outputs
                .iter()
                .map(|port| mf_runtime::SubgraphStreamOutput {
                    node: mf_runtime::NodeId::new(0),
                    optional: !port.required,
                    port: port.clone(),
                    name: port.name.to_string(),
                })
                .collect();
            let flow = flow
                .with_stream_outputs(streams)
                .context(BackendBuildSnafu)?;
            let outputs = flow.execute_with_sink(&mut child, &mut sink)?;
            sink.decoder.accept(outputs)?;
            Ok(())
        })();
        if let Some(run) = child.observation_mut() {
            run.finish(
                result
                    .as_ref()
                    .err()
                    .map(|error| error as &dyn std::fmt::Display),
            );
        }
        result.boxed().context(BackendSnafu {
            outcome: TurnExit::Failed,
            reply: sink.decoder.result().cloned().map(Box::new),
        })?;
        sink.decoder.finish()
    }
}

impl EmbeddedBackend {
    fn invoke_stream(
        &self,
        request: ConversationInputs,
        context: &mut ExecutionContext,
        mut preview: Option<&mut dyn FnMut(crate::TextDelta) -> Result<(), ChatError>>,
        observer: Option<&Observer>,
    ) -> Result<ConversationResult, ChatError> {
        let arguments = self.contract.arguments(request, &self.parameters)?;
        let registry = NodeRegistry::from_inventory()?;
        let observation = observer
            .map(|observer| self.plan.start_stream_observation(observer, RunId::new()))
            .transpose()?;
        let instance = mf_compiler::instantiate_stream(&self.plan, &registry)?.start_with_options(
            mf_runtime::StreamOptions {
                arguments,
                observation,
                ..Default::default()
            },
        )?;
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
struct TitleGenerationContext {
    prompt: String,
    context: crate::contract::TitleContext,
    model: mf_runtime::ValueRef,
    base_url: mf_runtime::ValueRef,
    api_key: mf_runtime::ValueRef,
}

impl TitleGenerationContext {
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
                .get(&mf_runtime::output_id("params", name))
                .cloned()
                .context(InvalidSnafu {
                    field: "title parameters",
                    reason: "requires resolved model, base_url, and api_key inputs",
                })
        };
        Ok(Some(Self {
            prompt: request.prompt.clone(),
            context: context.title.clone(),
            model: parameter("model")?,
            base_url: parameter("base_url")?,
            api_key: parameter("api_key")?,
        }))
    }
}

#[derive(Clone, Debug, NodeValue)]
struct ConversationCompleted {
    database_path: String,
    invocation: Invocation,
    result: ConversationResult,
    title_context: Option<TitleGenerationContext>,
}

#[derive(Clone, Debug, NodeValue)]
struct InvocationEvent {
    preview: Option<PreviewEvent>,
    completed: Option<ConversationCompleted>,
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
        let title_context =
            TitleGenerationContext::from_inputs(&input.request, self.title_parameters.as_ref())?;
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
                            completed: None,
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
                    completed: Some(ConversationCompleted {
                        database_path: input.database_path,
                        invocation: input.invocation,
                        result: response,
                        title_context,
                    }),
                },
            }
            .into(),
        )
    }
}

#[derive(NodeValue)]
struct PrepareTitleInputs {
    completed: ConversationCompleted,
}

#[derive(Debug, Default, NodeValue)]
struct PreparedTitle {
    enabled: bool,
    prompt: Option<String>,
    text: Option<String>,
    status: Option<String>,
    context: Option<crate::contract::ConversationContext>,
    model: Option<mf_runtime::ValueRef>,
    base_url: Option<mf_runtime::ValueRef>,
    api_key: Option<mf_runtime::ValueRef>,
}

struct PrepareTitle;

impl mf_runtime::TypedTaskNode for PrepareTitle {
    type Input = PrepareTitleInputs;
    type Output = PreparedTitle;

    fn execute(
        &self,
        input: Self::Input,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
        let completed = input.completed;
        let enabled = completed.title_context.is_some()
            && completed.result.status == "succeeded"
            && completed.result.title.is_none()
            && !completed.result.usages.iter().any(|usage| {
                usage
                    .attribution
                    .as_ref()
                    .and_then(|attribution| attribution.purpose.as_deref())
                    == Some("title")
            });
        if !enabled {
            return Ok(TypedNodeResult {
                outputs: PreparedTitle::default(),
                skipped: [
                    "prompt", "text", "status", "context", "model", "base_url", "api_key",
                ]
                .map(String::from)
                .into(),
                loop_summary: None,
            });
        }
        let title = completed
            .title_context
            .expect("enabled title has generation context");
        Ok(PreparedTitle {
            enabled: true,
            prompt: Some(title.prompt),
            text: Some(completed.result.text),
            status: Some(completed.result.status),
            context: Some(crate::contract::ConversationContext {
                title: title.context,
            }),
            model: Some(title.model),
            base_url: Some(title.base_url),
            api_key: Some(title.api_key),
        }
        .into())
    }
}

register_task!("mf_chat.title.prepare", PrepareTitle);

#[derive(NodeValue)]
struct FinishConversationInputs {
    completed: ConversationCompleted,
    title: mf_runtime::Nullable<String>,
    title_usages: Vec<crate::CallUsage>,
}

impl FinishConversationInputs {
    fn into_turn_completion(self) -> Result<CompleteTurnInputs, ChatError> {
        let mut response = self.completed.result;
        if let mf_runtime::Nullable::Value(title) = self.title {
            response.title = Some(title);
        }
        response.usages.extend(self.title_usages);
        response.validate()?;
        Ok(CompleteTurnInputs {
            database_path: self.completed.database_path,
            invocation: self.completed.invocation,
            response,
        })
    }
}

struct CompletionCapture {
    calls: Arc<Mutex<crate::CallEventCollector>>,
    delivered: Arc<Mutex<Option<ConversationResult>>>,
}

#[derive(Default)]
struct FinishConversation {
    capture: Option<CompletionCapture>,
}

impl mf_runtime::TypedTaskNode for FinishConversation {
    type Input = FinishConversationInputs;
    type Output = SettlementOutputs;

    fn execute(
        &self,
        input: Self::Input,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
        let mut completion = input.into_turn_completion()?;
        if let Some(capture) = &self.capture {
            capture
                .calls
                .lock()
                .expect("call events were not poisoned")
                .apply(&mut completion.response.usages);
            *capture
                .delivered
                .lock()
                .expect("delivery state was not poisoned") = Some(completion.response.clone());
        }
        Ok(settle(completion)?.into())
    }
}

register_task!("mf_chat.conversation.finish", FinishConversation::default());

#[derive(Debug, NodeValue)]
struct RoutedEvent {
    preview: Option<PreviewEvent>,
    completed: Option<ConversationCompleted>,
}

#[derive(Default)]
struct RouteInvocation {
    completed: Option<ConversationCompleted>,
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
                    self.completed.is_none(),
                    InvalidSnafu {
                        field: "conversation event",
                        reason: "cannot follow the final response"
                    }
                );
                ensure!(
                    input.event.preview.is_some() != input.event.completed.is_some(),
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
                            completed: None,
                        },
                        vec!["completed".into()],
                    )
                } else {
                    self.completed = input.event.completed;
                    return Ok(Default::default());
                }
            }
            mf_runtime::NodeEvent::UpstreamClosed => {
                let completed = self.completed.take().context(InvalidSnafu {
                    field: "conversation event",
                    reason: "ended without a final response",
                })?;
                (
                    RoutedEvent {
                        preview: None,
                        completed: Some(completed),
                    },
                    vec!["preview".into()],
                )
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
        Some(usize::from(self.completed.is_some()))
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

#[derive(Clone, NodeValue)]
struct PreparedConversation {
    started: TurnStarted,
}

#[derive(serde::Deserialize)]
struct UnboundAdapter {
    inputs: Vec<mf_runtime::PortSpec>,
    outputs: Vec<mf_runtime::PortSpec>,
    #[serde(default)]
    references: Vec<String>,
}

impl mf_runtime::NodePortContract for UnboundAdapter {
    fn ports(&self) -> mf_runtime::NodePorts {
        mf_runtime::NodePorts {
            inputs: self.inputs.clone(),
            outputs: self.outputs.clone(),
        }
    }
}

impl mf_runtime::TaskNode for UnboundAdapter {
    fn execute(
        &self,
        _: mf_runtime::Inputs,
        _: &mut ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, mf_runtime::NodeExecutionError> {
        Err(InvalidSnafu {
            field: "embedded conversation adapter",
            reason: "must be bound by the application host",
        }
        .build()
        .into())
    }
}

fn prepare_adapter(config: serde_json::Value) -> Result<PreparedNode, mf_runtime::NodeBuildError> {
    let adapter: UnboundAdapter = mf_runtime::deserialize_config(config)?;
    let metadata = NodeMetadata {
        context_references: adapter
            .references
            .iter()
            .map(|output| mf_runtime::ContextReference::new(output, "conversation result"))
            .collect(),
        ..Default::default()
    };
    PreparedNode::new(adapter, metadata)
}

inventory::submit! {
    mf_runtime::NodeRegistration {
        kind: "mf_chat.workflow.prepare",
        factory: mf_runtime::NodeFactory::Plain(prepare_adapter),
    }
}

inventory::submit! {
    mf_runtime::NodeRegistration {
        kind: "mf_chat.workflow.complete",
        factory: mf_runtime::NodeFactory::Plain(prepare_adapter),
    }
}

struct PrepareWorkflow {
    ports: mf_runtime::NodePorts,
    contract: ChatContract,
    parameters: WorkflowArguments,
    automatic_titles: bool,
}

impl mf_runtime::NodePortContract for PrepareWorkflow {
    fn ports(&self) -> mf_runtime::NodePorts {
        self.ports.clone()
    }
}

impl mf_runtime::TaskNode for PrepareWorkflow {
    fn execute(
        &self,
        inputs: mf_runtime::Inputs,
        _: &mut ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, mf_runtime::NodeExecutionError> {
        let mut started = TurnStarted::from_values(inputs).map_err(ChatError::from)?;
        if let Some(context) = &mut started.request.context {
            context.title.enabled = self.automatic_titles;
        }
        let mut outputs = self
            .contract
            .arguments(started.request.clone(), &self.parameters)?
            .0;
        outputs.extend(
            PreparedConversation { started }
                .into_values()
                .map_err(ChatError::from)?,
        );
        Ok(mf_runtime::NodeResult {
            outputs,
            ..Default::default()
        })
    }
}

struct CompleteWorkflow {
    outputs: Vec<String>,
    title_parameters: Option<WorkflowArguments>,
    capture: CompletionCapture,
}

impl mf_runtime::TypedTaskNode for CompleteWorkflow {
    type Input = PreparedConversation;
    type Output = InvocationEmission;

    fn execute(
        &self,
        input: Self::Input,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
        let mut values = mf_runtime::Outputs::new();
        // Optional skipped outputs must not skip settlement through data dependencies.
        for name in &self.outputs {
            if let mf_runtime::ContextValue::Value(value) =
                context.output(&mf_runtime::output_id("workflow", name))?
            {
                values.insert(name.clone(), value.clone());
            }
        }
        let mut response = ConversationResult::decode(values)?;
        self.capture
            .calls
            .lock()
            .expect("call events were not poisoned")
            .apply(&mut response.usages);
        if self.title_parameters.is_none() {
            *self
                .capture
                .delivered
                .lock()
                .expect("delivery state was not poisoned") = Some(response.clone());
        }
        Ok(InvocationEmission {
            event: InvocationEvent {
                preview: None,
                completed: Some(ConversationCompleted {
                    title_context: TitleGenerationContext::from_inputs(
                        &input.started.request,
                        self.title_parameters.as_ref(),
                    )?,
                    database_path: input.started.database_path,
                    invocation: input.started.invocation,
                    result: response,
                }),
            },
        }
        .into())
    }
}

#[derive(NodeValue)]
struct WorkflowPreviewInputs {
    answer: crate::TextDelta,
}

#[derive(Default)]
struct WorkflowPreview {
    invocation: Option<Invocation>,
}

impl mf_runtime::TypedTaskNode for WorkflowPreview {
    type Input = WorkflowPreviewInputs;
    type Output = PreviewOutput;

    fn execute(
        &self,
        input: Self::Input,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
        let invocation = self.invocation.clone().context(InvalidSnafu {
            field: "conversation preview",
            reason: "must be bound by the application host",
        })?;
        let preview = PreviewEvent {
            invocation,
            delta: input.answer,
        };
        preview.validate()?;
        Ok(PreviewOutput { preview }.into())
    }
}

register_task!("mf_chat.workflow.preview", WorkflowPreview::default());

struct ConversationPlan {
    plan: CompiledWorkflow,
    bindings: Vec<(&'static str, PreparedNode)>,
}

fn compose_conversation(
    plan: &CompiledWorkflow,
    parameters: &WorkflowArguments,
    automatic_titles: bool,
    invocation: &Invocation,
    title_parameters: Option<WorkflowArguments>,
    capture: CompletionCapture,
    registry: &NodeRegistry,
) -> Result<ConversationPlan, ChatError> {
    let contract = ChatContract::from_manifest(&plan.manifest(registry)?)?;
    let call = compile_call(&plan.definition, registry)?.context(InvalidSnafu {
        field: "embedded workflow",
        reason: "requires a finite Flow without internal stream dataflow",
    })?;
    let call_node = mf_compiler::prepare_definition(&call.definition, registry)?
        .0
        .remove(0);
    let targets: std::collections::BTreeSet<_> = parameters
        .0
        .keys()
        .map(String::as_str)
        .chain(contract.input_targets())
        .collect();
    let argument_ports: Vec<_> = call_node
        .metadata
        .ports
        .inputs
        .iter()
        .filter(|port| targets.contains(port.name.as_ref()))
        .cloned()
        .collect();
    let mut ports = mf_runtime::NodePorts {
        inputs: TurnStarted::ports(),
        outputs: PreparedConversation::ports(),
    };
    ports.outputs.extend(argument_ports.clone());
    let result_outputs: Vec<_> = call_node
        .metadata
        .ports
        .outputs
        .iter()
        .filter(|port| {
            ConversationResult::ports()
                .iter()
                .any(|expected| expected.name == port.name)
        })
        .map(|port| port.name.to_string())
        .collect();
    let metadata = NodeMetadata {
        context_references: result_outputs
            .iter()
            .map(|name| {
                mf_runtime::ContextReference::new(
                    mf_runtime::output_id("workflow", name),
                    "conversation result",
                )
            })
            .collect(),
        ..Default::default()
    };
    let mut definition =
        serde_json::to_value(flows::conversation::plan()?.definition).context(JsonSnafu)?;
    let nodes = definition["nodes"]
        .as_array_mut()
        .expect("conversation nodes");
    let workflow = nodes
        .iter_mut()
        .find(|node| node["id"] == "workflow")
        .expect("conversation invocation");
    let inputs = workflow["inputs"].take();
    *workflow = serde_json::json!({
        "id":"workflow", "kind":"workflow.call", "config":call.definition.nodes[0].config,
        "inputs": argument_ports.iter().map(|port| (port.name.to_string(), serde_json::json!({
            "from":{"node":"prepare_workflow","port":port.name}
        }))).collect::<serde_json::Map<_, _>>()
    });
    nodes.push(serde_json::json!({
        "id":"prepare_workflow", "kind":"mf_chat.workflow.prepare", "config":ports, "inputs":inputs
    }));
    nodes.push(serde_json::json!({
        "id":"complete_workflow", "kind":"mf_chat.workflow.complete",
        "config":{
            "inputs":PreparedConversation::ports(), "outputs":InvocationEmission::ports(),
            "references":metadata.context_references.iter().map(|reference| &reference.output).collect::<Vec<_>>()
        },
        "inputs":{"started":{"from":{"node":"prepare_workflow","port":"started"}}}
    }));
    let route = nodes
        .iter_mut()
        .find(|node| node["id"] == "route")
        .expect("conversation route");
    route["inputs"]["event"]["from"]["node"] = "complete_workflow".into();
    let mut bindings = Vec::new();
    // A subscription preserves the authored streaming request even without a UI consumer.
    if contract.answer_stream {
        nodes.push(serde_json::json!({
            "id":"preview", "kind":"mf_chat.workflow.preview", "inputs":{
                "answer":{"from":{"node":"workflow","port":"answer"}}
            }
        }));
        bindings.push((
            "preview",
            PreparedNode::typed_task(
                WorkflowPreview {
                    invocation: Some(invocation.clone()),
                },
                NodeMetadata::default(),
            )
            .context(BackendBuildSnafu)?,
        ));
        definition["outputs"]
            .as_array_mut()
            .expect("conversation outputs")
            .iter_mut()
            .find(|output| output["name"] == "preview")
            .expect("conversation preview")["node"] = "preview".into();
    }
    for (index, port) in call_node
        .metadata
        .stream_outputs
        .iter()
        .filter(|port| port.name != "answer")
        .enumerate()
    {
        definition["outputs"].as_array_mut().expect("conversation outputs")
            .push(serde_json::json!({
                "name":format!("workflow_stream_{index}"), "node":"workflow", "port":port.name, "optional":true
            }));
    }
    definition["edges"]
        .as_array_mut()
        .expect("conversation edges")
        .push(serde_json::json!({
            "from_node":"workflow", "to_node":"complete_workflow"
        }));
    let definition = serde_json::from_value(definition).context(JsonSnafu)?;
    let plan = mf_compiler::compile_resolved_definition(&definition, registry)?;
    let prepare = PreparedNode::new(
        PrepareWorkflow {
            ports,
            contract,
            parameters: parameters.clone(),
            automatic_titles,
        },
        NodeMetadata::default(),
    )
    .context(BackendBuildSnafu)?;
    let complete = PreparedNode::typed_task(
        CompleteWorkflow {
            outputs: result_outputs,
            title_parameters,
            capture,
        },
        metadata,
    )
    .context(BackendBuildSnafu)?;
    bindings.extend([
        ("prepare_workflow", prepare),
        ("complete_workflow", complete),
    ]);
    Ok(ConversationPlan { plan, bindings })
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
    let registry = NodeRegistry::from_inventory()?;
    let delivered = Arc::new(Mutex::new(None));
    let ConversationPlan { mut plan, bindings } =
        if let Some((embedded, parameters)) = backend.embedded_workflow() {
            compose_conversation(
                embedded,
                parameters,
                automatic_titles,
                &invocation,
                title_parameters,
                CompletionCapture {
                    calls: Arc::clone(&calls),
                    delivered: Arc::clone(&delivered),
                },
                &registry,
            )?
        } else {
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
            ConversationPlan {
                plan: flows::conversation::plan()?,
                bindings: vec![("workflow", bound)],
            }
        };
    let (mut nodes, order) = mf_compiler::prepare_definition(&plan.definition, &registry)?;
    for (id, bound) in bindings {
        let node = nodes
            .iter_mut()
            .find(|node| node.definition_id.as_str() == id)
            .expect("conversation declares the bound adapter");
        *node = FlowNode::new(node.definition_id.clone(), bound);
    }
    let completion = nodes
        .iter_mut()
        .find(|node| node.definition_id.as_str() == "finish")
        .expect("application conversation declares finalization");
    *completion = FlowNode::new(
        completion.definition_id.clone(),
        PreparedNode::typed_task(
            FinishConversation {
                capture: Some(CompletionCapture {
                    calls,
                    delivered: Arc::clone(&delivered),
                }),
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
    fn embedded_composition_keeps_resolved_credentials_out_of_the_definition() {
        let mut definition =
            WorkflowDefinition::from_json(include_str!("../../../examples/chat-flow.json"))
                .unwrap();
        for dependency in definition.dependencies.values_mut() {
            dependency.path = Some("missing-host-provider".into());
        }
        let profile = WorkflowProfile::new(
            "default",
            definition,
            WorkflowArguments::try_from(serde_json::json!({"params.api_key":"private-credential"}))
                .unwrap(),
        )
        .unwrap();
        let (plan, parameters) = profile.backend.embedded_workflow().unwrap();
        let composed = compose_conversation(
            plan,
            parameters,
            false,
            &Invocation::new("session"),
            None,
            CompletionCapture {
                calls: Default::default(),
                delivered: Default::default(),
            },
            &NodeRegistry::from_inventory().unwrap(),
        )
        .unwrap();
        assert!(
            !serde_json::to_string(&composed.plan)
                .unwrap()
                .contains("private-credential")
        );
        let prepare = composed
            .bindings
            .into_iter()
            .find(|(id, _)| *id == "prepare_workflow")
            .unwrap()
            .1;
        let result = prepare
            .execution
            .as_task_node()
            .unwrap()
            .execute(
                TurnStarted {
                    database_path: "chat.sqlite".into(),
                    invocation: Invocation::new("session"),
                    request: ConversationInputs {
                        prompt: "Hello".into(),
                        system: None,
                        history: None,
                        context: None,
                    },
                }
                .into_values()
                .unwrap(),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        let key = &result.outputs["params.api_key"];
        assert!(key.is_secret());
        assert!(key.ptr_eq(&parameters.0["params.api_key"]));
    }

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
                preview: None,
                completed: Some(ConversationCompleted {
                    database_path: "chat.sqlite".into(),
                    invocation: Invocation::new("session"),
                    title_context: None,
                    result: ConversationResult {
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
                        completed: None,
                    },
                }),
                &context,
            )
            .unwrap();
        assert_eq!(effects.emissions.len(), 1);
        assert_eq!(effects.emissions[0].result.outputs.preview, Some(preview));
        assert_eq!(
            effects.emissions[0].result.skipped,
            ["completed".into()].into()
        );
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
                .completed
                .as_ref()
                .unwrap()
                .result
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
                ("params.model".into(), "model".into()),
                ("params.base_url".into(), "http://localhost/v1".into()),
                (
                    "params.api_key".into(),
                    mf_runtime::ValueRef::from("credential").into_secret(),
                ),
            ]
            .into(),
        );
        event.event.completed.as_mut().unwrap().title_context =
            TitleGenerationContext::from_inputs(&request, Some(&parameters)).unwrap();
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
        let completed = closed
            .emissions
            .into_iter()
            .next()
            .unwrap()
            .result
            .outputs
            .completed
            .unwrap();
        let output = PrepareTitle
            .execute(
                PrepareTitleInputs { completed },
                &mut ExecutionContext::default(),
            )
            .unwrap()
            .outputs;
        assert!(output.enabled);
        assert_eq!(output.prompt.as_deref(), Some("Prompt"));
        assert_eq!(output.text.as_deref(), Some("canonical"));
        let api_key = output.api_key.as_ref().unwrap();
        assert_eq!(api_key.as_str(), Some("credential"));
        assert!(api_key.is_secret());
        assert!(!output.model.as_ref().unwrap().is_secret());
    }

    #[test]
    fn title_preparation_preserves_workflow_owned_title_results() {
        for reported_call in [false, true] {
            let mut completed = final_event().event.completed.unwrap();
            completed.title_context = Some(TitleGenerationContext {
                prompt: "Prompt".into(),
                context: crate::contract::TitleContext {
                    source: "provisional".into(),
                    revision: 0,
                    attempted: false,
                    enabled: true,
                },
                model: "model".into(),
                base_url: "http://localhost/v1".into(),
                api_key: mf_runtime::ValueRef::from("credential").into_secret(),
            });
            if reported_call {
                completed.result.usages.push(serde_json::from_value(serde_json::json!({
                    "call_id":"existing-title", "model":"model", "attribution":{"purpose":"title"}
                })).unwrap());
            } else {
                completed.result.title = Some("Workflow title".into());
            }
            let prepared = PrepareTitle
                .execute(
                    PrepareTitleInputs { completed },
                    &mut ExecutionContext::default(),
                )
                .unwrap();
            assert!(!prepared.outputs.enabled);
            assert!(prepared.skipped.contains("prompt"));
            assert!(prepared.skipped.contains("api_key"));
            assert_eq!(
                prepared
                    .outputs
                    .into_values()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                vec!["enabled"]
            );
        }
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
                            completed: None,
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
        let host = crate::ChatHost::open(
            directory.path().join("chat.sqlite"),
            crate::HostConfig::new(profile),
        )
        .unwrap();
        let session = host
            .database()
            .create_session("custom", None, None)
            .unwrap();
        host.submit(&session.id, "first").unwrap().join().unwrap();
        host.submit(&session.id, "second").unwrap().join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        assert_eq!(
            host.database()
                .messages(&session.id, 10, None)
                .unwrap()
                .messages
                .len(),
            4
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
