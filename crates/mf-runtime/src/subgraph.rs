use crate::{ExecutionContext, FlowOutputs, NodeIdentity, PortSpec, WorkflowRunError};
use snafu::{ResultExt, Snafu};
use std::sync::{Arc, mpsc};

#[derive(Debug, Snafu)]
enum ForwardingError {
    #[snafu(display("subgraph stream forwarding channel closed: {source}"))]
    Send { source: mpsc::SendError<()> },
    #[snafu(display("subgraph stream acknowledgement closed: {source}"))]
    Acknowledge { source: mpsc::RecvError },
    #[snafu(display("could not start subgraph stream coordinator: {source}"))]
    Coordinator { source: std::io::Error },
}

struct Emission {
    result: crate::NodeResult,
    acknowledge: mpsc::SyncSender<Result<(), crate::StreamError>>,
}

/// A cloneable producer endpoint for concurrent prepared-body invocations.
///
/// Each send waits until the caller-owned sink has accepted or rejected the emission.
/// Delivery failures are propagated to the producer and cancel the enclosing execution.
#[derive(Clone)]
pub struct SubgraphStreamSender {
    id: crate::DefinitionId,
    sender: mpsc::SyncSender<Emission>,
    cancellation: crate::StreamCancellation,
}

impl crate::EmissionSink for SubgraphStreamSender {
    fn send(&mut self, result: crate::NodeResult) -> Result<(), crate::StreamError> {
        let result: Result<(), crate::NodeExecutionError> = (|| {
            let (acknowledge, reply) = mpsc::sync_channel(0);
            self.sender
                .send(Emission {
                    result,
                    acknowledge,
                })
                // A disconnected channel needs no payload in its diagnostic.
                .map_err(|_| mpsc::SendError(()))
                .context(SendSnafu)
                .boxed()?;
            reply.recv().context(AcknowledgeSnafu).boxed()??;
            Ok(())
        })();
        result
            .context(crate::StreamProducerSnafu {
                definition_id: self.id.clone(),
            })
            .inspect_err(|error| self.cancellation.cancel(error.clone()))
    }
}

/// Forwards prepared-body emissions through one caller-owned sink.
///
/// Use this sink directly for sequential bodies. Concurrent bodies can call
/// [`SubgraphStream::forward`] and clone the supplied [`SubgraphStreamSender`]
/// for their workers. In both modes, delivery failures cancel the enclosing
/// execution instead of becoming recoverable body or item failures.
pub struct SubgraphStream<'a> {
    id: crate::DefinitionId,
    sink: &'a mut dyn crate::EmissionSink,
    cancellation: crate::StreamCancellation,
}

impl<'a> SubgraphStream<'a> {
    pub fn new(
        id: impl Into<crate::DefinitionId>,
        context: &ExecutionContext,
        sink: &'a mut dyn crate::EmissionSink,
    ) -> Self {
        Self {
            id: id.into(),
            sink,
            cancellation: context.cancellation(),
        }
    }

    /// Fans concurrent producers into the borrowed sink on the calling thread.
    ///
    /// The rendezvous channel preserves backpressure without buffering a body's
    /// complete stream. Producer and coordinator panics retain their normal unwind.
    pub fn forward<T>(
        &mut self,
        run: impl FnOnce(SubgraphStreamSender) -> Result<T, crate::NodeExecutionError> + Send,
    ) -> Result<T, crate::NodeExecutionError>
    where
        T: Send,
    {
        std::thread::scope(|scope| {
            let (sender, receiver) = mpsc::sync_channel::<Emission>(0);
            let sender = SubgraphStreamSender {
                id: self.id.clone(),
                sender,
                cancellation: self.cancellation.clone(),
            };
            let worker = std::thread::Builder::new()
                .name(format!("subgraph-stream-{}", self.id))
                .spawn_scoped(scope, || run(sender))
                .context(CoordinatorSnafu)
                .boxed()?;
            for emission in receiver {
                let result = crate::EmissionSink::send(self, emission.result);
                let _ = emission.acknowledge.send(result);
            }
            worker
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        })
    }
}

impl crate::EmissionSink for SubgraphStream<'_> {
    fn is_connected(&self) -> bool {
        self.sink.is_connected()
    }

    fn send(&mut self, result: crate::NodeResult) -> Result<(), crate::StreamError> {
        self.sink
            .send(result)
            .inspect_err(|error| self.cancellation.cancel(error.clone()))
    }
}

type SubgraphBody = dyn Fn(&mut ExecutionContext, &mut dyn crate::EmissionSink) -> Result<FlowOutputs, WorkflowRunError>
    + Send
    + Sync
    + 'static;

#[derive(Clone)]
pub struct PreparedSubgraph {
    pub nodes: Vec<NodeIdentity>,
    pub inputs: crate::WorkflowInputSchema,
    pub outputs: Vec<PortSpec>,
    pub stream_outputs: Vec<PortSpec>,
    body: Arc<SubgraphBody>,
}

/// A dynamic container that executes one prepared body with optional stream forwarding.
///
/// Prepare implementations through [`crate::PreparedNode::subgraph`]. The runtime
/// adapts the same execution method to terminal and streaming tasks and reflects
/// stream outputs from [`SubgraphNode::body`]. Scope creation, invocation count,
/// and result aggregation remain node-specific policies.
pub trait SubgraphNode: crate::NodePortContract + Send + Sync {
    fn body(&self) -> &PreparedSubgraph;

    fn supports_success_dependencies(&self) -> bool {
        true
    }

    fn execute(
        &self,
        inputs: crate::Inputs,
        context: &mut ExecutionContext,
        sink: &mut dyn crate::EmissionSink,
    ) -> Result<crate::NodeResult, crate::NodeExecutionError>;
}

pub(crate) struct SubgraphTask<N>(pub N);

impl<N: SubgraphNode> crate::NodePortContract for SubgraphTask<N> {
    fn ports(&self) -> crate::NodePorts {
        self.0.ports()
    }

    fn stream_outputs(&self) -> Vec<PortSpec> {
        self.0.body().stream_outputs.clone()
    }
}

impl<N: SubgraphNode> crate::TaskNode for SubgraphTask<N> {
    fn supports_success_dependencies(&self) -> bool {
        self.0.supports_success_dependencies()
    }

    fn execute(
        &self,
        inputs: crate::Inputs,
        context: &mut ExecutionContext,
    ) -> Result<crate::NodeResult, crate::NodeExecutionError> {
        self.0.execute(inputs, context, &mut crate::DiscardSink)
    }

    fn execute_with_sink(
        &self,
        inputs: crate::Inputs,
        context: &mut ExecutionContext,
        sink: &mut dyn crate::EmissionSink,
    ) -> Result<crate::NodeResult, crate::NodeExecutionError> {
        self.0.execute(inputs, context, sink)
    }
}

impl<N: SubgraphNode> crate::TaskStreamNode for SubgraphTask<N> {
    fn supports_success_dependencies(&self) -> bool {
        self.0.supports_success_dependencies()
    }

    fn execute(
        &self,
        inputs: crate::Inputs,
        context: &mut ExecutionContext,
        emitter: &mut crate::Emitter<'_>,
    ) -> Result<crate::NodeResult, crate::NodeExecutionError> {
        self.0.execute(inputs, context, emitter)
    }
}

impl PreparedSubgraph {
    pub fn new(
        nodes: Vec<NodeIdentity>,
        outputs: Vec<PortSpec>,
        body: impl Fn(&mut ExecutionContext) -> Result<FlowOutputs, WorkflowRunError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::streaming(nodes, outputs, Vec::new(), move |state, _| body(state))
    }

    pub fn streaming(
        nodes: Vec<NodeIdentity>,
        outputs: Vec<PortSpec>,
        stream_outputs: Vec<PortSpec>,
        body: impl Fn(
            &mut ExecutionContext,
            &mut dyn crate::EmissionSink,
        ) -> Result<FlowOutputs, WorkflowRunError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            nodes,
            inputs: Default::default(),
            outputs,
            stream_outputs,
            body: Arc::new(body),
        }
    }

    pub fn with_input_schema(mut self, inputs: crate::WorkflowInputSchema) -> Self {
        self.inputs = inputs;
        self
    }

    pub fn execute_in_context(
        &self,
        state: &mut ExecutionContext,
    ) -> Result<FlowOutputs, WorkflowRunError> {
        self.execute_with_sink(state, &mut crate::DiscardSink)
    }

    pub fn execute_with_sink(
        &self,
        state: &mut ExecutionContext,
        sink: &mut dyn crate::EmissionSink,
    ) -> Result<FlowOutputs, WorkflowRunError> {
        (self.body)(state, sink)
    }
}

/// One frozen child emission selected by a containing scope.
#[derive(Clone, Debug)]
pub struct SubgraphStreamOutput {
    pub node: crate::NodeId,
    pub port: PortSpec,
    pub name: String,
    pub optional: bool,
}

pub(super) struct ProjectedSink<'a> {
    pub sink: &'a mut dyn crate::EmissionSink,
    pub outputs: &'a [SubgraphStreamOutput],
    pub node: crate::NodeId,
    pub definition_id: &'a crate::DefinitionId,
}

impl crate::EmissionSink for ProjectedSink<'_> {
    fn is_connected(&self) -> bool {
        self.sink.is_connected()
    }

    fn send(&mut self, result: crate::NodeResult) -> Result<(), crate::StreamError> {
        let mut projected = crate::NodeResult::default();
        for output in self.outputs {
            if output.node == self.node
                && let Some(value) = result.outputs.get(output.port.name.as_ref())
            {
                projected.outputs.insert(output.name.clone(), value.clone());
            } else {
                if output.node == self.node && !output.optional {
                    return Err(crate::NodeExecutionError::from(
                        crate::runner::InvalidOutputSnafu {
                            definition_id: self.definition_id.clone(),
                            message: format!(
                                "missing required exported stream output `{}`",
                                output.name
                            ),
                        }
                        .build(),
                    ))
                    .context(crate::stream_instance::ProducerSnafu {
                        definition_id: self.definition_id.clone(),
                    });
                }
                projected.skipped.insert(output.name.clone());
            }
        }
        self.sink.send(projected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EmissionSink, NodeId, NodeResult, ValueRef, ValueType};

    #[test]
    fn projection_preserves_nested_sensitivity_and_marks_other_children_skipped() {
        struct Collect(Option<NodeResult>);
        impl EmissionSink for Collect {
            fn send(&mut self, result: NodeResult) -> Result<(), crate::StreamError> {
                self.0 = Some(result);
                Ok(())
            }
        }
        let payload = ValueRef::object([(
            "token".into(),
            ValueRef::from("private-value").into_secret(),
        )]);
        let outputs = vec![
            SubgraphStreamOutput {
                node: NodeId::new(0),
                port: PortSpec::new("value", ValueType::Object, true),
                name: "selected".into(),
                optional: false,
            },
            SubgraphStreamOutput {
                node: NodeId::new(1),
                port: PortSpec::new("value", ValueType::String, true),
                name: "other".into(),
                optional: false,
            },
        ];
        let mut collected = Collect(None);
        let mut sink = ProjectedSink {
            sink: &mut collected,
            outputs: &outputs,
            node: NodeId::new(0),
            definition_id: &"child".into(),
        };
        assert!(sink.is_connected());
        sink.send(crate::Outputs::from([("value".into(), payload.clone())]).into())
            .unwrap();
        let result = collected.0.unwrap();
        assert_eq!(result.outputs["selected"], payload);
        assert_eq!(
            serde_json::to_value(result.outputs["selected"].redacted()).unwrap(),
            serde_json::json!({"token":"[REDACTED]"})
        );
        assert!(!result.outputs.contains_key("other"));
        assert!(result.skipped.contains("other"));
    }
}
