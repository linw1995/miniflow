use crate::{
    ExecutionContext, FlowNode, Inputs, NodeExecutionError, NodeMetadata, NodePorts, NodeResult,
    PortSpec, StreamCancellation, StreamError, TaskNode, TypedNodeResult,
};
use mf_telemetry::event::LoopPathEntry;
use snafu::{OptionExt, ResultExt, ensure};
use std::{
    marker::PhantomData,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// One business invocation with incremental values and an authoritative final result.
pub trait TaskStreamNode: Send + Sync {
    fn supports_success_dependencies(&self) -> bool {
        true
    }

    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<NodeResult, NodeExecutionError>;
}

pub trait TypedTaskStreamNode: Send + Sync {
    fn supports_success_dependencies(&self) -> bool {
        true
    }

    type Input: crate::NodeInputs;
    type Output: crate::NodeOutputs;
    type Emission: crate::NodeOutputs;

    fn execute(
        &self,
        input: Self::Input,
        context: &mut ExecutionContext,
        emitter: &mut TypedEmitter<'_, '_, Self::Emission>,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError>;
}

/// Delivery policy is separate from output conversion and validation.
pub trait EmissionSink {
    fn is_connected(&self) -> bool {
        true
    }

    fn send(&mut self, result: NodeResult) -> Result<(), StreamError>;

    fn send_with_cancellation(
        &mut self,
        result: NodeResult,
        cancellation: &StreamCancellation,
    ) -> Result<(), StreamError> {
        if let Some(error) = cancellation.failure() {
            return Err(error);
        }
        self.send(result)
    }

    fn set_deadline(&mut self, _deadline: std::time::Instant) {}
}

pub struct DiscardSink;

impl EmissionSink for DiscardSink {
    fn is_connected(&self) -> bool {
        false
    }

    fn send(&mut self, _: NodeResult) -> Result<(), StreamError> {
        Ok(())
    }
}

/// A borrowed, validated output sink for one invocation.
pub struct Emitter<'a> {
    contract: FlowNode<()>,
    cancellation: StreamCancellation,
    sink: &'a mut dyn EmissionSink,
    sink_validates: bool,
    snapshot: Option<(crate::SnapshotRecorder, Vec<LoopPathEntry>)>,
}

impl<'a> Emitter<'a> {
    pub fn new(
        node: impl Into<crate::DefinitionId>,
        ports: &[PortSpec],
        cancellation: StreamCancellation,
        sink: &'a mut dyn EmissionSink,
    ) -> Self {
        Self {
            contract: FlowNode {
                definition_id: node.into(),
                node: (),
                metadata: NodeMetadata::new(NodePorts {
                    inputs: vec![],
                    outputs: ports.to_vec(),
                }),
                output_keys: Default::default(),
            },
            cancellation,
            sink,
            sink_validates: false,
            snapshot: None,
        }
    }

    pub(super) fn with_sink_validation(
        node: impl Into<crate::DefinitionId>,
        ports: &[PortSpec],
        cancellation: StreamCancellation,
        sink: &'a mut dyn EmissionSink,
    ) -> Self {
        // Native stream sinks validate each send and own fatal instance state.
        let mut emitter = Self::new(node, ports, cancellation, sink);
        emitter.sink_validates = true;
        emitter
    }

    pub fn is_connected(&self) -> bool {
        self.sink.is_connected()
    }

    pub fn send(&mut self, result: NodeResult) -> Result<(), NodeExecutionError> {
        let snapshot = self.snapshot.as_ref().map(|_| result.outputs.clone());
        self.admit(result)?;
        if let (Some((recorder, scope)), Some(outputs)) = (&self.snapshot, snapshot) {
            recorder.record(crate::SnapshotEvent {
                scope: scope.clone(),
                node: self.contract.definition_id.as_str().into(),
                snapshot: crate::NodeSnapshot {
                    inputs: crate::snapshot::ports_snapshot(&Inputs::new()),
                    outputs: crate::snapshot::ports_snapshot(&outputs),
                    skipped: Arc::from([]),
                    outcome: crate::SnapshotOutcome::Emitted,
                    error: None,
                },
            });
        }
        Ok(())
    }

    pub fn send_with_deadline(
        &mut self,
        result: NodeResult,
        deadline: std::time::Instant,
    ) -> Result<(), NodeExecutionError> {
        self.sink.set_deadline(deadline);
        let original = self.cancellation.clone();
        self.cancellation = original.with_deadline(deadline);
        let result = self.send(result);
        self.cancellation = original;
        result
    }

    pub fn set_snapshot_context(&mut self, context: &ExecutionContext) {
        self.snapshot = context
            .snapshot_recorder()
            .map(|recorder| (recorder.clone(), context.invocation_path()));
    }

    fn admit(&mut self, result: NodeResult) -> Result<(), NodeExecutionError> {
        self.check_cancellation()?;
        let node = self.contract.definition_id.as_str();
        if !self.sink_validates {
            crate::ExecutionOutputs::validate_result(&self.contract, &result)?;
            for port in &self.contract.metadata.ports.outputs {
                ensure!(
                    !port.required || result.outputs.contains_key(port.name.as_ref()),
                    crate::runner::InvalidOutputSnafu {
                        definition_id: node,
                        message: format!("missing required output `{}`", port.name),
                    }
                );
            }
        }
        Ok(self
            .sink
            .send_with_cancellation(result, &self.cancellation)?)
    }

    fn check_cancellation(&self) -> Result<(), NodeExecutionError> {
        if let Some(error) = self.cancellation.failure() {
            return Err(error.into());
        }
        Ok(())
    }
}

pub struct TypedEmitter<'a, 'sink, O> {
    emitter: &'a mut Emitter<'sink>,
    output: PhantomData<fn(O)>,
}

impl<'a, 'sink, O: crate::NodeOutputs> TypedEmitter<'a, 'sink, O> {
    pub fn new(emitter: &'a mut Emitter<'sink>) -> Self {
        Self {
            emitter,
            output: PhantomData,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.emitter.is_connected()
    }

    pub fn send_with_deadline(
        &mut self,
        result: impl Into<TypedNodeResult<O>>,
        deadline: std::time::Instant,
    ) -> Result<(), NodeExecutionError> {
        let result = crate::encode_typed_result(result.into())?;
        self.emitter.send_with_deadline(result, deadline)
    }

    pub fn send(
        &mut self,
        result: impl Into<TypedNodeResult<O>>,
    ) -> Result<(), NodeExecutionError> {
        let result = crate::encode_typed_result(result.into())?;
        self.emitter.send(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StreamInvocation {
    pub id: u64,
    pub node: String,
    pub path: Vec<LoopPathEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamOutputContract {
    pub name: String,
    pub path: Vec<crate::DefinitionId>,
    pub port: String,
    pub value_type: crate::ValueType,
    pub required: bool,
}

impl StreamInvocation {
    pub fn new(context: &ExecutionContext) -> Result<Self, StreamError> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .ok()
            .context(crate::stream_instance::ResourceSnafu {
                message: "stream invocation identities exhausted",
            })?;
        Ok(Self {
            id,
            node: context.current_node().unwrap_or("<task-stream>").into(),
            path: context.invocation_path(),
        })
    }
}

pub struct TaskStreamExecutor {
    node: Box<dyn TaskStreamNode>,
    outputs: Vec<PortSpec>,
    exports: Vec<StreamOutputContract>,
}

impl TaskStreamExecutor {
    pub fn new(node: Box<dyn TaskStreamNode>, outputs: Vec<PortSpec>) -> Self {
        Self {
            node,
            outputs,
            exports: Vec::new(),
        }
    }

    pub fn set_exports(&mut self, exports: Vec<StreamOutputContract>) {
        self.exports = exports;
    }

    pub fn set_stream_contract(&mut self, outputs: &[PortSpec]) {
        self.outputs = outputs.to_vec();
    }

    pub fn exports(&self) -> &[StreamOutputContract] {
        &self.exports
    }

    pub fn execute_with_sink(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        sink: &mut dyn EmissionSink,
    ) -> Result<NodeResult, NodeExecutionError> {
        crate::live::with_exports(&self.exports, context, sink, |context, sink| {
            self.invoke(inputs, context, sink)
        })
    }

    fn invoke(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        sink: &mut dyn EmissionSink,
    ) -> Result<NodeResult, NodeExecutionError> {
        let mut emitter = Emitter::new(
            context.current_node().unwrap_or("<task-stream>"),
            &self.outputs,
            context.cancellation(),
            sink,
        );
        emitter.set_snapshot_context(context);
        emitter.check_cancellation()?;
        let result = self.node.execute(inputs, context, &mut emitter);
        emitter.check_cancellation()?;
        result
    }
}

impl TaskNode for TaskStreamExecutor {
    fn supports_success_dependencies(&self) -> bool {
        self.node.supports_success_dependencies()
    }

    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        self.execute_with_sink(inputs, context, &mut DiscardSink)
    }
}

pub(super) struct TypedTaskStreamAdapter<N>(pub N);

impl<N: TypedTaskStreamNode> TaskStreamNode for TypedTaskStreamAdapter<N> {
    fn supports_success_dependencies(&self) -> bool {
        self.0.supports_success_dependencies()
    }

    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<NodeResult, NodeExecutionError> {
        let input = <N::Input as crate::NodeInputs>::from_inputs(inputs)
            .context(crate::node::InputDecodeSnafu)?;
        let result = self
            .0
            .execute(input, context, &mut TypedEmitter::new(emitter));
        crate::encode_typed_result(result?)
    }
}
