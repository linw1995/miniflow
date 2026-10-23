use crate::{
    ExecutionContext, FlowNode, Inputs, NodeExecutionError, NodeMetadata, NodePorts, NodeResult,
    PortSpec, StreamCancellation, StreamError, TaskNode, TypedNodeResult,
};
use mf_telemetry::event::LoopPathEntry;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    marker::PhantomData,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// One business invocation with incremental values and an authoritative final result.
pub trait TaskStreamNode: Send + Sync {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<NodeResult, NodeExecutionError>;
}

pub trait TypedTaskStreamNode: Send + Sync {
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

    fn fail(&mut self, _error: EmissionError) {}
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

#[derive(Clone, Debug, Snafu)]
pub enum EmissionError {
    #[snafu(display("invalid stream output from `{node}`: {source}"))]
    Validation {
        node: String,
        #[snafu(source(from(crate::WorkflowRunError, Arc::new)))]
        source: Arc<crate::WorkflowRunError>,
    },
    #[snafu(display("could not encode stream output from `{node}`: {source}"))]
    Encoding {
        node: String,
        #[snafu(source(from(NodeExecutionError, Arc::new)))]
        source: Arc<NodeExecutionError>,
    },
    #[snafu(display("stream delivery from `{node}` failed: {source}"))]
    Delivery { node: String, source: StreamError },
    #[snafu(display("stream output from `{node}` is missing required port `{port}`"))]
    Missing { node: String, port: String },
}

impl From<EmissionError> for NodeExecutionError {
    fn from(error: EmissionError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(Box::new(error)).into()
    }
}

/// A borrowed sink which remembers publication failures even if a provider ignores them.
pub struct Emitter<'a> {
    contract: FlowNode<()>,
    cancellation: StreamCancellation,
    sink: &'a mut dyn EmissionSink,
    failure: Option<EmissionError>,
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
            failure: None,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.sink.is_connected()
    }

    pub fn send(&mut self, result: NodeResult) -> Result<(), NodeExecutionError> {
        let outcome = self.admit(result);
        self.remember(outcome)
    }

    fn admit(&mut self, result: NodeResult) -> Result<(), EmissionError> {
        self.check()?;
        let node = self.contract.definition_id.as_str();
        crate::ExecutionOutputs::validate_result(&self.contract, &result)
            .context(ValidationSnafu { node })?;
        for port in &self.contract.metadata.ports.outputs {
            ensure!(
                !port.required || result.outputs.contains_key(port.name.as_ref()),
                MissingSnafu {
                    node,
                    port: port.name.to_string(),
                }
            );
        }
        self.sink.send(result).context(DeliverySnafu { node })
    }

    fn check(&self) -> Result<(), EmissionError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if let Some(error) = self.cancellation.failure() {
            return Err(error).context(DeliverySnafu {
                node: self.contract.definition_id.as_str(),
            });
        }
        Ok(())
    }

    fn remember(&mut self, result: Result<(), EmissionError>) -> Result<(), NodeExecutionError> {
        result.map_err(|error| {
            if self.failure.is_none() {
                self.sink.fail(error.clone());
                self.failure = Some(error.clone());
            }
            error.into()
        })
    }

    pub fn finish(&self) -> Result<(), NodeExecutionError> {
        self.check().map_err(NodeExecutionError::from)
    }

    pub fn failure(&self) -> Option<&EmissionError> {
        self.failure.as_ref()
    }

    fn encoding_failed(&mut self, error: NodeExecutionError) -> NodeExecutionError {
        let result = Err(error).context(EncodingSnafu {
            node: self.contract.definition_id.as_str(),
        });
        self.remember(result).unwrap_err()
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
        let result = crate::encode_typed_result(result.into())
            .map_err(|error| self.emitter.encoding_failed(error))?;
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
        let mut emitter = Emitter::new(
            context.current_node().unwrap_or("<task-stream>"),
            &self.outputs,
            context.cancellation(),
            sink,
        );
        emitter.finish()?;
        let result = self.node.execute(inputs, context, &mut emitter);
        emitter.finish()?;
        result
    }
}

impl TaskNode for TaskStreamExecutor {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let Some(publisher) = context
            .live_publisher()
            .filter(|_| !self.exports.is_empty())
            .cloned()
        else {
            return self.execute_with_sink(inputs, context, &mut DiscardSink);
        };
        let mut sink = crate::live::ExportSink {
            publisher,
            exports: &self.exports,
            invocation: StreamInvocation::new(context)?,
            cancellation: context.cancellation(),
        };
        sink.open()?;
        let result = self.execute_with_sink(inputs, context, &mut sink);
        let closed = sink.close(result.is_ok());
        match result {
            Ok(result) => {
                closed?;
                Ok(result)
            }
            Err(error) => Err(error),
        }
    }
}

pub(super) struct TypedTaskStreamAdapter<N>(pub N);

impl<N: TypedTaskStreamNode> TaskStreamNode for TypedTaskStreamAdapter<N> {
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
        emitter.finish()?;
        crate::encode_typed_result(result?)
    }
}
