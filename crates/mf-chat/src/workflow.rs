//! Application workflow hosting over the shared conversation protocol.

use crate::{
    ChatDatabase, ChatError, Completion, ConversationInputs, ConversationResult, Invocation,
    PreviewConsumer, PreviewEvent,
    contract::{ChatContract, CompletionState, ReconciledCompletion, TurnExit},
    error::*,
    flows,
    nodes::{BeginTurnInputs, CompleteTurnInputs, TurnStarted},
};
use mf_compiler::{CompiledWorkflow, WorkflowDefinition};
use mf_runtime::{
    EmissionSink, ExecutionContext, FlowNode, NodeMetadata, NodeRegistry, NodeValue, PortSpec,
    PreparedNode, PreparedSubgraph, StreamCancellation, WorkflowArguments, WorkflowInput,
    WorkflowInputSchema,
};
use mf_telemetry::{identity::RunId, observation::Observer};
use snafu::{IntoError, OptionExt, ResultExt, ensure};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
    task::{Wake, Waker},
};

const CONTEXT_WINDOW_PROBE_TIMEOUT_MS: u64 = 5_000;

type ContextWindowCacheMap = BTreeMap<(String, String), Option<i64>>;

/// Host-scoped cache of provider-advertised context windows keyed by `(base_url, model)`.
#[derive(Clone, Debug, Default)]
pub struct ModelContextWindowCache {
    entries: Arc<Mutex<ContextWindowCacheMap>>,
}

impl ModelContextWindowCache {
    /// Returns the cached context window lookup state for `(base_url, model)`.
    ///
    /// Returns `None` if no lookup has been cached yet, `Some(None)` if the endpoint did not
    /// advertise a positive window, and `Some(Some(tokens))` if a positive window is known.
    pub fn get(&self, base_url: &str, model: &str) -> Option<Option<u64>> {
        let normalized = base_url.trim_end_matches('/');
        if normalized.is_empty() {
            return None;
        }
        self.entries
            .lock()
            .expect("context window cache was not poisoned")
            .get(&(normalized.to_owned(), model.to_owned()))
            .copied()
            .map(|value| {
                value
                    .and_then(|tokens| u64::try_from(tokens).ok())
                    .filter(|&tokens| tokens > 0)
            })
    }

    /// Records a single provider-advertised context window lookup outcome for `(base_url, model)`.
    pub fn record(&self, base_url: &str, model: &str, context_window: Option<u64>) {
        let normalized = base_url.trim_end_matches('/');
        if normalized.is_empty() {
            return;
        }
        let stored = context_window
            .and_then(|tokens| i64::try_from(tokens).ok())
            .filter(|&tokens| tokens > 0);
        self.entries
            .lock()
            .expect("context window cache was not poisoned")
            .insert((normalized.to_owned(), model.to_owned()), stored);
    }

    /// Records provider-advertised context windows returned by a `/models` listing.
    pub fn record_models(&self, base_url: &str, models: &[mfn_llm::ModelInfo]) {
        let normalized = base_url.trim_end_matches('/');
        if normalized.is_empty() {
            return;
        }
        let mut entries = self
            .entries
            .lock()
            .expect("context window cache was not poisoned");
        for model in models {
            entries.insert(
                (normalized.to_owned(), model.id.clone()),
                model.context_window.filter(|&tokens| tokens > 0),
            );
        }
    }

    fn resolve(
        &self,
        config: &crate::CompactionConfig,
        parameters: Option<&WorkflowArguments>,
        context: &ExecutionContext,
    ) -> crate::CompactionContext {
        if !config.enabled || config.context_window.is_some() {
            return config.resolved_context(None);
        }
        let Some(probe) = model_info_probe_inputs(parameters) else {
            return config.resolved_context(None);
        };
        let key = (
            probe.base_url.trim_end_matches('/').to_owned(),
            probe.model.clone(),
        );
        if let Some(cached) = self
            .entries
            .lock()
            .expect("context window cache was not poisoned")
            .get(&key)
            .copied()
        {
            return config.resolved_context(cached);
        }
        let discovered = match execute_model_info_node(probe, context) {
            Ok(window) => window.filter(|&tokens| tokens > 0),
            Err(()) => {
                if context.cancellation().failure().is_some() {
                    // Never cache a cancellation-interrupted probe as an absent window.
                    return config.resolved_context(None);
                }
                None
            }
        };
        self.entries
            .lock()
            .expect("context window cache was not poisoned")
            .insert(key, discovered);
        config.resolved_context(discovered)
    }
}

fn execute_model_info_node(
    probe: ModelInfoProbeInputs,
    parent: &ExecutionContext,
) -> Result<Option<i64>, ()> {
    let node = prepare_model_info_node().ok_or(())?;
    let source: FlowNode<()> = FlowNode {
        definition_id: "params".into(),
        node: (),
        metadata: NodeMetadata {
            ports: mf_runtime::NodePorts {
                inputs: Vec::new(),
                outputs: vec![
                    mf_runtime::PortSpec::new("model", mf_runtime::ValueType::String, true),
                    mf_runtime::PortSpec::new("base_url", mf_runtime::ValueType::String, true),
                    mf_runtime::PortSpec::new("api_key", mf_runtime::ValueType::String, true),
                ],
            },
            ..Default::default()
        },
        output_keys: OnceLock::new(),
    };
    let mut context = ExecutionContext::for_message(
        &source,
        mf_runtime::NodeResult {
            outputs: [
                ("model".into(), mf_runtime::ValueRef::from(probe.model)),
                (
                    "base_url".into(),
                    mf_runtime::ValueRef::from(probe.base_url),
                ),
                (
                    "api_key".into(),
                    mf_runtime::ValueRef::from(probe.api_key.unwrap_or_default()).into_secret(),
                ),
            ]
            .into(),
            ..Default::default()
        },
    )
    .map_err(|_| ())?;
    context.set_cancellation(parent.cancellation());
    let task = node.execution.as_task_node().ok_or(())?;
    let executed = task
        .execute(mf_runtime::Inputs::new(), &mut context)
        .map_err(|_| ())?;
    let outputs =
        <mfn_llm::ModelInfoOutputs as NodeValue>::from_values(executed.outputs).map_err(|_| ())?;
    Ok(outputs.context_window)
}

struct ModelInfoProbeInputs {
    model: String,
    base_url: String,
    api_key: Option<String>,
}

fn prepare_model_info_node() -> Option<PreparedNode> {
    let registry = NodeRegistry::from_inventory().ok()?;
    let factory = registry.get(mfn_llm::MODEL_INFO_KIND)?;
    factory
        .instantiate(serde_json::json!({
            "model": {"$ref": "params.model"},
            "base_url": {"$ref": "params.base_url"},
            "api_key": {"$ref": "params.api_key"},
            "timeout_ms": CONTEXT_WINDOW_PROBE_TIMEOUT_MS,
        }))
        .ok()
}

fn model_info_probe_inputs(parameters: Option<&WorkflowArguments>) -> Option<ModelInfoProbeInputs> {
    let parameters = parameters?;
    let model = parameters
        .0
        .get(&mf_runtime::output_id("params", "model"))?
        .as_str()?
        .to_owned();
    let base_url = parameters
        .0
        .get(&mf_runtime::output_id("params", "base_url"))?
        .as_str()?
        .to_owned();
    let api_key = parameters
        .0
        .get(&mf_runtime::output_id("params", "api_key"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Some(ModelInfoProbeInputs {
        model,
        base_url,
        api_key,
    })
}

struct CancelWorkflow(StreamCancellation);

impl Wake for CancelWorkflow {
    fn wake(self: Arc<Self>) {
        self.0.cancel(
            mf_runtime::StreamExecutionSnafu {
                message: "conversation cancelled by host",
            }
            .build(),
        );
    }
}

/// Invokes one backend and forwards optional model-output previews into the conversation Flow.
pub trait ConversationBackend: Send + Sync {
    /// Exposes a finite embedded Flow and resolved parameters for `workflow.call` composition.
    /// Other backends are invoked through the host executor for that call.
    fn embedded_workflow(&self) -> Option<(&CompiledWorkflow, &WorkflowArguments)> {
        None
    }

    /// Exposes an executable Flow's interface for host registration through `workflow.call`.
    /// `invoke` implements the call body; the host does not plan the executable's sources.
    fn registered_workflow(&self) -> Option<(&mf_runtime::WorkflowManifest, &WorkflowArguments)> {
        None
    }

    /// Resolved inputs for the optional built-in title Flow, with its `params` namespace.
    fn builtin_title_parameters(&self) -> Option<WorkflowArguments> {
        None
    }

    /// Resolved inputs for the optional built-in compaction Flow, with its `params` namespace.
    fn builtin_compaction_parameters(&self) -> Option<WorkflowArguments> {
        self.builtin_title_parameters()
    }

    fn invoke(
        &self,
        invocation: &Invocation,
        request: ConversationInputs,
        context: &mut ExecutionContext,
        preview: Option<&mut dyn FnMut(crate::PreviewDelta) -> Result<(), ChatError>>,
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
        let (nodes, _) =
            mf_compiler::prepare_compiled_graph_with_layouts(&plan.graph, registry, None)?;
        for output in plan
            .graph
            .definition
            .outputs
            .iter()
            .filter(|output| matches!(output.name.as_ref(), "append" | "replace_context"))
        {
            let producer = nodes
                .iter()
                .find(|node| node.definition_id == output.node)
                .expect("compiled output producer");
            let port = producer
                .metadata
                .ports
                .outputs
                .iter()
                .find(|port| port.name == output.port)
                .expect("compiled output port");
            ensure!(
                port.value_type
                    .compatibility_with(&mf_runtime::ValueType::Object)
                    != mf_runtime::TypeCompatibility::Incompatible,
                InvalidSnafu {
                    field: "chat outputs",
                    reason: "result alternatives require object-valued producers"
                }
            );
        }
        let parameters =
            mf_runtime::resolve_workflow_inputs(&contract.parameters, &parameters, |name| {
                std::env::var_os(name)
            })?;
        Self::with_backend(
            name,
            Arc::new(EmbeddedBackend {
                call: compile_call(&plan.graph.definition, registry)?,
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
        matches!(output.name.as_ref(), "answer" | "tool_call_delta")
            && !streams.contains(&(output.node.as_str(), output.port.as_ref()))
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
    Ok(Some(compile_call_wrapper(
        definition,
        "conversation",
        registry,
    )?))
}

fn compile_call_wrapper(
    definition: &WorkflowDefinition,
    name: &str,
    registry: &NodeRegistry,
) -> Result<CompiledWorkflow, ChatError> {
    let outputs: Vec<_> = definition.outputs.iter()
        .filter(|output| ConversationResult::ports().iter().any(|port| port.name == output.name))
        .map(|output| serde_json::json!({
        "name": output.name, "node": "workflow", "port": output.name, "optional": output.optional
    })).collect();
    let definition = serde_json::from_value(serde_json::json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [{"id": "workflow", "kind": "workflow.call", "config": {
            "flow": name, "body": definition
        }}],
        "outputs": outputs
    }))
    .context(JsonSnafu)?;
    Ok(mf_compiler::compile_resolved_definition(
        &definition,
        registry,
    )?)
}

struct ConversationSink<'a> {
    decoder: crate::contract::ConversationOutputDecoder,
    preview: Option<&'a mut dyn FnMut(crate::PreviewDelta) -> Result<(), ChatError>>,
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

    fn builtin_compaction_parameters(&self) -> Option<WorkflowArguments> {
        self.contract
            .builtin_compaction_parameters(&self.parameters)
    }

    fn invoke(
        &self,
        _: &Invocation,
        request: ConversationInputs,
        context: &mut ExecutionContext,
        preview: Option<&mut dyn FnMut(crate::PreviewDelta) -> Result<(), ChatError>>,
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
        mut preview: Option<&mut dyn FnMut(crate::PreviewDelta) -> Result<(), ChatError>>,
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
                snapshots: context.snapshot_recorder().cloned(),
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
    pub completion: Completion,
}

#[derive(Clone, Debug, NodeValue)]
pub struct SettlementOutputs {
    pub settlement: Settlement,
}

struct TitleGenerationContext {
    prompt: String,
    context: crate::contract::ConversationContext,
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
            prompt: request.prompt()?.to_owned(),
            context: context.clone(),
            model: parameter("model")?,
            base_url: parameter("base_url")?,
            api_key: parameter("api_key")?,
        }))
    }
}

struct CompactionGenerationContext {
    messages: Vec<mfn_llm::ChatMessage>,
    context: crate::contract::ConversationContext,
    model: mf_runtime::ValueRef,
    base_url: mf_runtime::ValueRef,
    api_key: mf_runtime::ValueRef,
}

impl CompactionGenerationContext {
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
                    field: "compaction parameters",
                    reason: "requires resolved model, base_url, and api_key inputs",
                })
        };
        Ok(Some(Self {
            messages: request.messages.clone(),
            context: context.clone(),
            model: parameter("model")?,
            base_url: parameter("base_url")?,
            api_key: parameter("api_key")?,
        }))
    }
}

struct InvocationPolicy {
    automatic_titles: bool,
    automatic_compaction: crate::CompactionConfig,
    context_windows: ModelContextWindowCache,
    probe_parameters: Option<WorkflowArguments>,
    title_parameters: Option<WorkflowArguments>,
    compaction_parameters: Option<WorkflowArguments>,
}

impl InvocationPolicy {
    fn resolve_context(
        &self,
        request: &mut ConversationInputs,
        context: &ExecutionContext,
    ) -> Result<(), ChatError> {
        if let Some(admitted) = &mut request.context {
            admitted.title.enabled = self.automatic_titles;
            admitted.compaction = self.context_windows.resolve(
                &self.automatic_compaction,
                self.probe_parameters.as_ref(),
                context,
            );
        }
        context
            .check_future_limits(None)
            .map_err(mf_runtime::NodeExecutionError::from)?;
        Ok(())
    }

    fn builds_auxiliary_result(&self) -> bool {
        self.title_parameters.is_some() || self.compaction_parameters.is_some()
    }
}

#[derive(NodeValue)]
struct PreviewOutput {
    preview: PreviewEvent,
}

fn emit_preview(
    invocation: &Invocation,
    delta: crate::PreviewDelta,
    sink: &mut dyn EmissionSink,
) -> Result<(), ChatError> {
    let preview = PreviewEvent {
        invocation: invocation.clone(),
        delta,
    };
    preview.validate()?;
    sink.send(PreviewOutput { preview }.into_values()?.into())?;
    Ok(())
}

enum InvocationTarget {
    Embedded {
        body: Box<PreparedSubgraph>,
        contract: ChatContract,
        parameters: WorkflowArguments,
    },
    Runner {
        contract: ChatContract,
        input_schema: WorkflowInputSchema,
        parameters: WorkflowArguments,
    },
    Custom,
}

impl InvocationTarget {
    fn alternatives(&self) -> Option<crate::ResultAlternatives> {
        match self {
            Self::Embedded { contract, .. } | Self::Runner { contract, .. } => {
                Some(contract.alternatives)
            }
            Self::Custom => None,
        }
    }
}

struct InvocationCall {
    backend: Arc<dyn ConversationBackend>,
    target: InvocationTarget,
    policy: InvocationPolicy,
    observer: Observer,
    capture: CompletionCapture,
    previews: bool,
}

impl InvocationCall {
    fn prepare(self) -> PreparedSubgraph {
        let nodes = match &self.target {
            InvocationTarget::Embedded { body, .. } => body.nodes.clone(),
            _ => Vec::new(),
        };
        PreparedSubgraph::streaming(
            nodes,
            InvocationOutputs::ports(),
            PreviewOutput::ports(),
            move |context, sink| host_call_error("workflow", self.execute(context, sink)),
        )
        .with_input_schema(host_input_schema(TurnStarted::ports()))
    }

    fn execute(
        &self,
        context: &mut ExecutionContext,
        sink: &mut dyn EmissionSink,
    ) -> Result<mf_runtime::FlowOutputs, ChatError> {
        let mut started: TurnStarted = host_arguments(context)?;
        self.policy.resolve_context(&mut started.request, context)?;
        let title_context = TitleGenerationContext::from_inputs(
            &started.request,
            self.policy.title_parameters.as_ref(),
        )?;
        let compaction_context = CompactionGenerationContext::from_inputs(
            &started.request,
            self.policy.compaction_parameters.as_ref(),
        )?;
        let mut consume = |delta| emit_preview(&started.invocation, delta, sink);
        let response = match &self.target {
            InvocationTarget::Embedded {
                body,
                contract,
                parameters,
            } => {
                let arguments = contract.arguments(started.request.clone(), parameters)?;
                context.set_workflow_arguments(arguments);
                let mut sink = ConversationSink {
                    decoder: crate::contract::ConversationOutputDecoder::new(contract),
                    preview: Some(&mut consume),
                };
                let outputs = body.invoke_with_sink(context, &mut sink)?;
                sink.decoder.accept(outputs)?;
                sink.decoder.finish()
            }
            target => {
                let (request, subscribe) = if let InvocationTarget::Runner {
                    contract,
                    input_schema,
                    parameters,
                } = target
                {
                    let mut arguments = contract.arguments(started.request.clone(), parameters)?;
                    input_schema.mark_secrets(&mut arguments);
                    let request = contract.conversation_inputs(&arguments)?;
                    context.set_workflow_arguments(arguments);
                    context.bind_workflow_inputs(input_schema)?;
                    (request, contract.has_previews())
                } else {
                    (started.request.clone(), self.previews)
                };
                self.backend.invoke(
                    &started.invocation,
                    request,
                    context,
                    subscribe.then_some(&mut consume as &mut dyn FnMut(_) -> _),
                    Some(&self.observer),
                )
            }
        };
        if let Err(error) = &response
            && let Some((_, Some(reply))) = backend_failure(error)
        {
            self.capture.retain(&mut reply.clone());
        }
        let mut response = response?;
        if let Some(alternatives) = self.target.alternatives() {
            alternatives.validate(&response)?;
            response.validate_against(&started.request.messages)?;
        } else {
            let mut connection =
                crate::storage::connect(std::path::Path::new(&started.database_path))?;
            let admitted = crate::turn::load_turn(&mut connection, &started.invocation.turn_id)?;
            crate::turn::validate_admitted(&mut connection, &admitted, &response)?;
        }
        if self.policy.builds_auxiliary_result() {
            self.capture.enrich(&mut response);
        } else {
            self.capture.retain(&mut response);
        }
        Ok(InvocationOutputs::from_completion(
            CompletedTurn {
                database_path: started.database_path,
                invocation: started.invocation,
                result: response,
            },
            title_context,
            compaction_context,
        )
        .into_values()?)
    }
}

#[derive(NodeValue)]
struct CompletedTurn {
    database_path: String,
    invocation: Invocation,
    result: ConversationResult,
}

#[derive(NodeValue)]
struct FinishConversationInputs {
    completed: CompletedTurn,
    title: mf_runtime::Nullable<String>,
    title_usages: Vec<crate::CallUsage>,
    compacted_messages: mf_runtime::Nullable<Vec<mfn_llm::ChatMessage>>,
    compaction_usages: Vec<crate::CallUsage>,
}

impl FinishConversationInputs {
    fn into_turn_completion(self) -> Result<CompleteTurnInputs, ChatError> {
        let mut response = self.completed.result;
        if let mf_runtime::Nullable::Value(title) = self.title {
            response.title = Some(title);
        }
        response.usages.extend(self.title_usages);
        if let mf_runtime::Nullable::Value(context_messages) = self.compacted_messages {
            let display_messages = match response.update {
                crate::MessageUpdate::Append { messages } => messages,
                crate::MessageUpdate::ReplaceContext {
                    display_messages, ..
                } => display_messages,
            };
            response.update = crate::MessageUpdate::ReplaceContext {
                context_messages,
                display_messages,
            };
        }
        response.usages.extend(self.compaction_usages);
        response.validate()?;
        Ok(CompleteTurnInputs {
            database_path: self.completed.database_path,
            invocation: self.completed.invocation,
            response,
        })
    }
}

#[derive(Clone)]
struct CompletionCapture {
    calls: Arc<Mutex<crate::CallEventCollector>>,
    delivered: Arc<Mutex<Option<ConversationResult>>>,
}

impl CompletionCapture {
    fn enrich(&self, result: &mut ConversationResult) {
        self.calls
            .lock()
            .expect("call events were not poisoned")
            .apply(&mut result.usages);
    }

    fn retain(&self, result: &mut ConversationResult) {
        self.enrich(result);
        *self
            .delivered
            .lock()
            .expect("delivery state was not poisoned") = Some(result.clone());
    }

    fn finish(&self, inputs: FinishConversationInputs) -> Result<SettlementOutputs, ChatError> {
        let mut completion = inputs.into_turn_completion()?;
        self.retain(&mut completion.response);
        let completion = crate::turn::complete(completion)?.completion;
        Ok(SettlementOutputs {
            settlement: Settlement { completion },
        })
    }
}

#[derive(NodeValue)]
struct AuxiliaryFacts {
    status: String,
    has_title: bool,
    appends_context: bool,
    title_configured: bool,
    compaction_configured: bool,
    purposes: Vec<String>,
}

#[derive(NodeValue)]
struct InvocationOutputs {
    completed: CompletedTurn,
    auxiliary: AuxiliaryFacts,
    title_prompt: Option<String>,
    title_text: Option<String>,
    title_context: Option<crate::ConversationContext>,
    title_model: Option<mf_runtime::ValueRef>,
    title_base_url: Option<mf_runtime::ValueRef>,
    title_api_key: Option<mf_runtime::ValueRef>,
    compact_messages: Option<Vec<mfn_llm::ChatMessage>>,
    compact_append_messages: Option<Vec<mfn_llm::ChatMessage>>,
    compact_usages: Option<Vec<crate::CallUsage>>,
    compact_context: Option<crate::ConversationContext>,
    compact_model: Option<mf_runtime::ValueRef>,
    compact_base_url: Option<mf_runtime::ValueRef>,
    compact_api_key: Option<mf_runtime::ValueRef>,
}

impl InvocationOutputs {
    fn from_completion(
        completed: CompletedTurn,
        title_context: Option<TitleGenerationContext>,
        compaction_context: Option<CompactionGenerationContext>,
    ) -> Self {
        let result = &completed.result;
        // CEL gates only need these facts. Keep credentials and transcript values on native edges.
        let auxiliary = AuxiliaryFacts {
            status: result.status.clone(),
            has_title: result.title.is_some(),
            appends_context: matches!(&result.update, crate::MessageUpdate::Append { .. }),
            title_configured: title_context.is_some(),
            compaction_configured: compaction_context.is_some(),
            purposes: result
                .usages
                .iter()
                .filter_map(|usage| usage.attribution.as_ref()?.purpose.clone())
                .collect(),
        };
        let mut outputs = Self {
            completed,
            auxiliary,
            title_prompt: None,
            title_text: None,
            title_context: None,
            title_model: None,
            title_base_url: None,
            title_api_key: None,
            compact_messages: None,
            compact_append_messages: None,
            compact_usages: None,
            compact_context: None,
            compact_model: None,
            compact_base_url: None,
            compact_api_key: None,
        };
        let result = &outputs.completed.result;
        if let Some(title) = title_context {
            outputs.title_prompt = Some(title.prompt);
            outputs.title_text = result.final_text().map(str::to_owned);
            outputs.title_context = Some(title.context);
            outputs.title_model = Some(title.model);
            outputs.title_base_url = Some(title.base_url);
            outputs.title_api_key = Some(title.api_key);
        }
        if let Some(compaction) = compaction_context
            && let crate::MessageUpdate::Append { messages } = &result.update
        {
            outputs.compact_messages = Some(compaction.messages);
            outputs.compact_append_messages = Some(messages.clone());
            outputs.compact_usages = Some(result.usages.clone());
            outputs.compact_context = Some(compaction.context);
            outputs.compact_model = Some(compaction.model);
            outputs.compact_base_url = Some(compaction.base_url);
            outputs.compact_api_key = Some(compaction.api_key);
        }
        outputs
    }
}

fn host_input_schema(ports: Vec<PortSpec>) -> WorkflowInputSchema {
    WorkflowInputSchema {
        inputs: [(
            "params".into(),
            ports
                .into_iter()
                .map(|port| {
                    (
                        port.name.into_owned(),
                        WorkflowInput {
                            value_type: port.value_type,
                            required: port.required,
                            binding: Default::default(),
                        },
                    )
                })
                .collect(),
        )]
        .into(),
        stdin: Default::default(),
    }
}

fn host_arguments<T: NodeValue>(context: &ExecutionContext) -> Result<T, ChatError> {
    let arguments = context.workflow_arguments().0.iter().map(|(name, value)| {
        let port = name.strip_prefix("params.").expect("chat host call input");
        (port.to_owned(), value.clone())
    });
    Ok(T::from_values(arguments.collect())?)
}

fn host_call_error<T>(
    id: &str,
    result: Result<T, ChatError>,
) -> Result<T, mf_runtime::WorkflowRunError> {
    result
        .map_err(mf_runtime::NodeExecutionError::from)
        .context(mf_runtime::NodeExecutionSnafu { definition_id: id })
}

fn host_task<I: NodeValue, O: NodeValue>(
    id: &'static str,
    execute: impl Fn(I) -> Result<O, ChatError> + Send + Sync + 'static,
) -> PreparedSubgraph {
    PreparedSubgraph::new(Vec::new(), O::ports(), move |context| {
        let result = (|| Ok(execute(host_arguments(context)?)?.into_values()?))();
        host_call_error(id, result)
    })
    .with_input_schema(host_input_schema(I::ports()))
}

fn bind_call(
    registry: &mut NodeRegistry,
    plan: &CompiledWorkflow,
    id: &str,
    body: PreparedSubgraph,
) -> Result<(), ChatError> {
    let child = &plan.graph.children[&mf_runtime::DefinitionId::from(id)]["body"];
    let name = child
        .relations
        .host
        .as_deref()
        .expect("chat host call name");
    registry.register_flow(name, child.definition.clone(), body)?;
    Ok(())
}

fn conversation_plan(
    registry: &NodeRegistry,
    embedded: Option<&CompiledWorkflow>,
) -> Result<CompiledWorkflow, ChatError> {
    let plan = flows::conversation::plan(registry)?;
    let Some(embedded) = embedded else {
        return Ok(plan);
    };
    let mut definition = plan.graph.definition;
    let workflow = definition
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "workflow")
        .expect("conversation declares the workflow call");
    workflow.config["body"] =
        serde_json::to_value(&embedded.graph.definition).context(JsonSnafu)?;
    Ok(mf_compiler::plan_resolved_definition(
        &definition,
        registry,
    )?)
}

fn backend_failure(error: &ChatError) -> Option<(TurnExit, Option<&ConversationResult>)> {
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = cause {
        if let Some(ChatError::Backend { outcome, reply, .. }) = error.downcast_ref::<ChatError>() {
            return Some((*outcome, reply.as_deref()));
        }
        cause = error.source();
    }
    None
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
    run_conversation_with_context_windows(
        database,
        request,
        backend,
        cancellation,
        automatic_titles,
        observer,
        preview,
        ModelContextWindowCache::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_conversation_with_context_windows(
    database: &ChatDatabase,
    request: BeginTurnInputs,
    backend: Arc<dyn ConversationBackend>,
    cancellation: StreamCancellation,
    automatic_titles: bool,
    observer: Option<&Observer>,
    preview: Option<PreviewConsumer>,
    context_windows: ModelContextWindowCache,
) -> Result<ReconciledCompletion, ChatError> {
    let automatic_compaction = crate::CompactionConfig::default();
    run_observed_conversation(
        database,
        request,
        backend,
        cancellation,
        automatic_titles,
        &automatic_compaction,
        observer,
        ConversationPresentation {
            preview,
            tools: None,
            context_windows,
        },
    )
}

pub(super) struct ConversationPresentation {
    pub preview: Option<PreviewConsumer>,
    pub tools: Option<crate::ToolExecutionMetrics>,
    pub context_windows: ModelContextWindowCache,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_observed_conversation(
    database: &ChatDatabase,
    mut request: BeginTurnInputs,
    backend: Arc<dyn ConversationBackend>,
    cancellation: StreamCancellation,
    automatic_titles: bool,
    automatic_compaction: &crate::CompactionConfig,
    observer: Option<&Observer>,
    presentation: ConversationPresentation,
) -> Result<ReconciledCompletion, ChatError> {
    automatic_compaction.validate()?;
    let ConversationPresentation {
        preview,
        tools,
        context_windows,
    } = presentation;
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
    let _execution = tools.as_ref().map(|tools| {
        let tools = tools.clone();
        mf_telemetry::observation::collect(move |record| tools.observe(record, false))
    });
    let snapshots = tools.map(|tools| {
        mf_runtime::SnapshotRecorder::new(move |event| {
            if let Some(run) = mf_telemetry::observation::current_run_id() {
                tools.observe_snapshot(run, &event.scope, &event.snapshot.inputs);
            }
        })
    });
    let title_parameters = automatic_titles
        .then(|| backend.builtin_title_parameters())
        .flatten();
    let observer = observer.cloned().unwrap_or_else(|| Observer::stderr(0));
    let mut registry = NodeRegistry::from_inventory()?;
    let registered = backend.registered_workflow();
    let embedded = registered
        .is_none()
        .then(|| backend.embedded_workflow())
        .flatten();
    let plan = conversation_plan(&registry, embedded.map(|(plan, _)| plan))?;
    let target = if let Some((manifest, parameters)) = registered {
        InvocationTarget::Runner {
            contract: ChatContract::from_manifest(manifest)?,
            input_schema: manifest.descriptor.input_schema.clone(),
            parameters: parameters.clone(),
        }
    } else if let Some((embedded, parameters)) = embedded {
        InvocationTarget::Embedded {
            body: Box::new(mf_compiler::prepare_call_body(
                &plan, &registry, "workflow",
            )?),
            contract: ChatContract::from_manifest(&embedded.manifest(&registry)?)?,
            parameters: parameters.clone(),
        }
    } else {
        InvocationTarget::Custom
    };
    let composed_alternatives = target.alternatives();
    let probe_parameters = automatic_compaction
        .enabled
        .then(|| {
            backend
                .builtin_compaction_parameters()
                .or_else(|| backend.builtin_title_parameters())
        })
        .flatten();
    let compaction_parameters = (automatic_compaction.enabled
        && composed_alternatives
            .is_none_or(|alternatives| alternatives == crate::ResultAlternatives::Append))
    .then(|| backend.builtin_compaction_parameters())
    .flatten();
    if let Some(alternatives) = composed_alternatives {
        request.result_alternatives = Some(if compaction_parameters.is_some() {
            match alternatives {
                crate::ResultAlternatives::Append => crate::ResultAlternatives::Both,
                other => other,
            }
        } else {
            alternatives
        });
    }
    let capture = CompletionCapture {
        calls,
        delivered: Arc::new(Mutex::new(None)),
    };
    bind_call(
        &mut registry,
        &plan,
        "begin",
        host_task("begin", crate::turn::begin),
    )?;
    bind_call(
        &mut registry,
        &plan,
        "workflow",
        InvocationCall {
            backend,
            target,
            policy: InvocationPolicy {
                automatic_titles,
                automatic_compaction: automatic_compaction.clone(),
                context_windows,
                probe_parameters,
                title_parameters,
                compaction_parameters,
            },
            observer: observer.clone(),
            capture: capture.clone(),
            previews: preview.is_some(),
        }
        .prepare(),
    )?;
    let finish = capture.clone();
    bind_call(
        &mut registry,
        &plan,
        "finish",
        host_task("finish", move |inputs| finish.finish(inputs)),
    )?;
    let flow = mf_compiler::instantiate_stream(&plan, &registry)?;
    let arguments = WorkflowArguments(
        request
            .into_values()?
            .into_iter()
            .map(|(name, value)| (mf_runtime::output_id("begin.params", &name), value))
            .collect(),
    );
    let observation = Some(plan.start_stream_observation(&observer, RunId::new())?);
    let result = (|| -> Result<Settlement, ChatError> {
        if let Some(error) = cancellation.failure() {
            return Err(error.into());
        }
        let instance = flow.start_with_options(mf_runtime::StreamOptions {
            arguments,
            observation,
            snapshots,
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
            let state = CompletionState::Committed(settlement.completion);
            Ok(ReconciledCompletion {
                state,
                diagnostics: vec![],
            })
        }
        Err(source) => {
            let (backend_outcome, retained) =
                backend_failure(&source).unwrap_or((TurnExit::Failed, None));
            let outcome = if cancellation.failure().is_some() {
                TurnExit::Cancelled
            } else {
                backend_outcome
            };
            let response = capture
                .delivered
                .lock()
                .expect("delivery state was not poisoned")
                .take()
                .or_else(|| retained.cloned());
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
    use mf_runtime::{TypedNodeResult, TypedTaskNode};
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
            Ok(
                ConversationResult::assistant(if context.output("begin.database_path").is_ok() {
                    "leaked"
                } else {
                    "isolated"
                })
                .into(),
            )
        }
    }

    inventory::submit! {
        mf_runtime::NodeRegistration {
            kind: "test.chat.context",
            factory: mf_runtime::NodeFactory::Plain(|_| {
                PreparedNode::typed_task(ReadContext, NodeMetadata::default())
            }),
        }
    }

    #[test]
    fn embedded_chat_binds_native_credentials_without_serializing_them() {
        use mfn_llm::test_support::{Reply, Server};
        let server = Server::start(vec![Reply::text("Answer")]);
        let mut definition =
            WorkflowDefinition::from_json(include_str!("../../../examples/chat-flow.json"))
                .unwrap();
        for dependency in definition.dependencies.values_mut() {
            dependency.path = Some("missing-host-provider".into());
        }
        let profile = WorkflowProfile::new(
            "default",
            definition,
            WorkflowArguments::try_from(serde_json::json!({
                "params.model": "fixture-model",
                "params.base_url": server.base_url,
                "params.api_key": "private-credential",
            }))
            .unwrap(),
        )
        .unwrap();
        let (embedded, _) = profile.backend.embedded_workflow().unwrap();
        let registry = NodeRegistry::from_inventory().unwrap();
        let plan = conversation_plan(&registry, Some(embedded)).unwrap();
        let description = mf_compiler::describe_compiled(&plan).unwrap();
        assert!(description.body(&["workflow".into()]).is_some());
        assert!(
            !serde_json::to_string(&plan.graph.definition)
                .unwrap()
                .contains("private-credential")
        );
        let mut config = crate::HostConfig::new(profile);
        config.automatic_titles = false;
        config.automatic_compaction.enabled = false;
        let root = tempfile::tempdir().unwrap();
        let host = crate::ChatHost::open(root.path().join("chat.sqlite"), config).unwrap();
        let session = host
            .database()
            .create_session("default", None, None)
            .unwrap();
        let completion = host
            .submit(&session.id, "Question")
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(completion.result.final_text(), Some("Answer"));
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .headers
                .to_ascii_lowercase()
                .contains("authorization: bearer private-credential")
        );
    }

    #[test]
    fn user_workflows_cannot_read_application_context_outputs() {
        let request = ConversationInputs {
            messages: vec![mfn_llm::ChatMessage::User {
                content: "hello".into(),
                metadata: None,
            }],
            context: None,
        };
        let invocation = Invocation::new("session");
        let source = FlowNode {
            definition_id: "begin".into(),
            node: (),
            metadata: NodeMetadata::new(mf_runtime::NodePorts {
                inputs: Vec::new(),
                outputs: TurnStarted::ports(),
            }),
            output_keys: OnceLock::new(),
        };
        let mut context = ExecutionContext::for_message(
            &source,
            TurnStarted {
                database_path: "private.sqlite".into(),
                invocation: invocation.clone(),
                request: request.clone(),
            }
            .into_values()
            .unwrap()
            .into(),
        )
        .unwrap();
        let definition = WorkflowDefinition::from_json(
            r#"{
            "version":"2026-10-19", "dependencies":{},
            "nodes":[{"id":"backend","kind":"test.chat.context"}],
            "outputs":[
                {"name":"append","node":"backend","port":"append"}
            ]
        }"#,
        )
        .unwrap();
        let profile = WorkflowProfile::new("custom", definition, Default::default()).unwrap();
        let result = profile
            .backend
            .invoke(&invocation, request, &mut context, None, None)
            .unwrap();
        assert_eq!(result.final_text(), Some("isolated"));
        assert!(context.output("begin.database_path").is_ok());
    }

    struct Backend(Arc<AtomicUsize>);

    impl ConversationBackend for Backend {
        fn invoke(
            &self,
            _: &Invocation,
            input: ConversationInputs,
            _: &mut ExecutionContext,
            _: Option<&mut dyn FnMut(crate::PreviewDelta) -> Result<(), ChatError>>,
            _: Option<&Observer>,
        ) -> Result<ConversationResult, ChatError> {
            assert!(input.context.is_some());
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(ConversationResult::assistant("answer"))
        }
    }

    fn completed_turn() -> CompletedTurn {
        CompletedTurn {
            database_path: "chat.sqlite".into(),
            invocation: Invocation::new("session"),
            result: ConversationResult::assistant("canonical"),
        }
    }

    #[test]
    fn terminal_outputs_keep_auxiliary_credentials_on_separate_native_ports() {
        let request = ConversationInputs {
            messages: vec![mfn_llm::ChatMessage::User {
                content: "Prompt".into(),
                metadata: None,
            }],
            context: Some(crate::contract::ConversationContext {
                model_context: crate::ModelContextRevision { revision: 0 },
                compaction: crate::CompactionContext::default(),
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
        let mut compact_parameters = parameters.clone();
        compact_parameters.0.insert(
            "params.api_key".into(),
            mf_runtime::ValueRef::from("compact-credential").into_secret(),
        );
        let title = TitleGenerationContext::from_inputs(&request, Some(&parameters)).unwrap();
        let compact =
            CompactionGenerationContext::from_inputs(&request, Some(&compact_parameters)).unwrap();
        let outputs = InvocationOutputs::from_completion(completed_turn(), title, compact)
            .into_values()
            .unwrap();
        assert!(!outputs.contains_key("preview"));
        assert_eq!(outputs["title_prompt"].as_str(), Some("Prompt"));
        assert_eq!(outputs["title_text"].as_str(), Some("canonical"));
        assert!(!outputs["auxiliary"].contains_secret());
        assert!(!outputs["completed"].contains_secret());
        for (port, parameters) in [
            ("title_api_key", &parameters),
            ("compact_api_key", &compact_parameters),
        ] {
            let api_key = &outputs[port];
            assert!(api_key.is_secret());
            assert!(api_key.ptr_eq(&parameters.0["params.api_key"]));
        }
        assert!(!outputs["title_model"].is_secret());
        assert!(!outputs["compact_model"].is_secret());
    }

    #[test]
    fn authored_title_gate_preserves_workflow_owned_title_results() {
        let gate = flows::conversation_definition()
            .unwrap()
            .nodes
            .into_iter()
            .find(|node| node.id.as_str() == "prepare_title")
            .unwrap();
        let registry = NodeRegistry::from_inventory().unwrap();
        let gate = registry
            .get(&gate.kind)
            .unwrap()
            .instantiate(gate.config)
            .unwrap();
        let gate = gate.execution.as_task_node().unwrap();
        for reported_call in [false, true] {
            let mut completed = completed_turn();
            let title_context = TitleGenerationContext {
                prompt: "Prompt".into(),
                context: crate::contract::ConversationContext {
                    model_context: crate::ModelContextRevision { revision: 0 },
                    compaction: crate::CompactionContext::default(),
                    title: crate::contract::TitleContext {
                        source: "provisional".into(),
                        revision: 0,
                        attempted: false,
                        enabled: true,
                    },
                },
                model: "model".into(),
                base_url: "http://localhost/v1".into(),
                api_key: mf_runtime::ValueRef::from("credential").into_secret(),
            };
            if reported_call {
                completed.result.usages.push(serde_json::from_value(serde_json::json!({
                    "call_id":"existing-title", "model":"model", "attribution":{"purpose":"title"}
                })).unwrap());
            } else {
                completed.result.title = Some("Workflow title".into());
            }
            let mut outputs =
                InvocationOutputs::from_completion(completed, Some(title_context), None)
                    .into_values()
                    .unwrap();
            let result = gate
                .execute(
                    [("facts".into(), outputs.remove("auxiliary").unwrap())].into(),
                    &mut ExecutionContext::default(),
                )
                .unwrap();
            assert_eq!(result.outputs["enabled"].as_bool(), Some(false));
            assert!(!result.outputs["enabled"].is_secret());
        }
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
            result_alternatives: None,
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

    #[test]
    fn context_window_cache_records_discovered_models_and_ignores_blank_base_url() {
        let cache = ModelContextWindowCache::default();
        cache.record_models(
            "/",
            &[mfn_llm::ModelInfo {
                id: "ignored".into(),
                context_window: Some(64_000),
                ..Default::default()
            }],
        );
        cache.record_models(
            "https://example.invalid/v1/",
            &[mfn_llm::ModelInfo {
                id: "cached-model".into(),
                context_window: Some(96_000),
                ..Default::default()
            }],
        );
        let args = WorkflowArguments(
            [
                (
                    mf_runtime::output_id("params", "base_url"),
                    mf_runtime::ValueRef::from("https://example.invalid/v1"),
                ),
                (
                    mf_runtime::output_id("params", "model"),
                    mf_runtime::ValueRef::from("cached-model"),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let resolved = cache.resolve(
            &crate::CompactionConfig::default(),
            Some(&args),
            &ExecutionContext::default(),
        );
        assert_eq!(resolved.context_window, 96_000);
    }
}
