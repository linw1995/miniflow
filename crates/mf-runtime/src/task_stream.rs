use crate::{
    ExecutionContext, FlowNode, Inputs, NodeExecutionError, NodeMetadata, NodePorts, NodeResult,
    PortSpec, StreamCancellation, StreamError, TaskNode, TypedNodeResult,
};
use mf_telemetry::event::LoopPathEntry;
use snafu::{ResultExt, ensure};
use std::{marker::PhantomData, sync::Arc};

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
        Ok(self.sink.send(result)?)
    }

    fn check_cancellation(&self) -> Result<(), NodeExecutionError> {
        if let Some(error) = self.cancellation.failure() {
            return Err(error.into());
        }
        Ok(())
    }
}

impl EmissionSink for Emitter<'_> {
    fn is_connected(&self) -> bool {
        Emitter::is_connected(self)
    }

    fn send(&mut self, result: NodeResult) -> Result<(), StreamError> {
        Emitter::send(self, result).context(crate::stream_instance::ProducerSnafu {
            definition_id: self.contract.definition_id.clone(),
        })
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

    pub fn send(
        &mut self,
        result: impl Into<TypedNodeResult<O>>,
    ) -> Result<(), NodeExecutionError> {
        let result = crate::encode_typed_result(result.into())?;
        self.emitter.send(result)
    }
}

pub struct TaskStreamExecutor {
    node: Box<dyn TaskStreamNode>,
    outputs: Vec<PortSpec>,
}

impl TaskStreamExecutor {
    pub fn new(node: Box<dyn TaskStreamNode>, outputs: Vec<PortSpec>) -> Self {
        Self { node, outputs }
    }

    pub fn set_stream_contract(&mut self, outputs: &[PortSpec]) {
        self.outputs = outputs.to_vec();
    }

    pub fn execute_with_sink(
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

    fn execute_with_sink(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        sink: &mut dyn EmissionSink,
    ) -> Result<NodeResult, NodeExecutionError> {
        TaskStreamExecutor::execute_with_sink(self, inputs, context, sink)
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
