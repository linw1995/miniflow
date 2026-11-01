use crate::{
    DefinitionId, ExecutionContext, Flow, FlowOutputs, NodeIdentity, PortSpec, WorkflowInputSchema,
    WorkflowRunError,
};
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

/// Startup input metadata for one weakly connected Flow within a subgraph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubgraphFlowInputMetadata {
    /// Node IDs in the subgraph's canonical execution order.
    pub nodes: Vec<DefinitionId>,
    /// The subset of the subgraph's startup interface owned by this Flow.
    pub inputs: WorkflowInputSchema,
}

/// Startup inputs partitioned by the independent Flows in a subgraph.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubgraphInputMetadata {
    /// Independent Flows in canonical execution order.
    pub flows: Vec<SubgraphFlowInputMetadata>,
}

impl SubgraphInputMetadata {
    /// Partitions a prepared Flow without changing its flat runtime input binding.
    pub fn from_flow(flow: &Flow) -> Self {
        let domains = flow.execution_domains().domains();
        let mut domain_flows = vec![usize::MAX; domains.len()];
        let mut components = Vec::<Vec<usize>>::new();
        for start in 0..domains.len() {
            if domain_flows[start] != usize::MAX {
                continue;
            }
            let component = components.len();
            components.push(Vec::new());
            let mut pending = vec![start];
            domain_flows[start] = component;
            while let Some(domain) = pending.pop() {
                components[component].push(domain);
                for adjacent in domains[domain]
                    .predecessors
                    .iter()
                    .chain(domains[domain].successors.iter())
                {
                    if domain_flows[*adjacent] == usize::MAX {
                        domain_flows[*adjacent] = component;
                        pending.push(*adjacent);
                    }
                }
            }
        }
        components.sort_by_key(|component| {
            component
                .iter()
                .map(|&domain| domains[domain].first_position)
                .min()
                .expect("a connected component contains a domain")
        });

        let mut node_flows = vec![usize::MAX; flow.nodes().len()];
        for (component, domain_ids) in components.iter().enumerate() {
            for &domain in domain_ids {
                for node in domains[domain].nodes.iter() {
                    node_flows[node.index()] = component;
                }
            }
        }
        let schema = flow.input_schema();
        let flows = components
            .iter()
            .enumerate()
            .map(|(component, _)| {
                let nodes: Vec<_> = flow
                    .execution_order()
                    .iter()
                    .filter(|node| node_flows[node.index()] == component)
                    .map(|node| flow.nodes()[node.index()].definition_id.clone())
                    .collect();
                let mut inputs = WorkflowInputSchema::default();
                for node in &nodes {
                    if let Some(ports) = schema.inputs.get(node.as_str()) {
                        inputs.inputs.insert(node.to_string(), ports.clone());
                    }
                    if let Some(requirement) = schema.stdin.get(node.as_str()) {
                        inputs.stdin.insert(node.to_string(), requirement.clone());
                    }
                }
                SubgraphFlowInputMetadata { nodes, inputs }
            })
            .collect();
        Self { flows }
    }
}

/// Compile-time interface facts. Deliberately contains no executable closure.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubgraphMetadata {
    pub inputs: crate::WorkflowInputSchema,
    pub outputs: Vec<PortSpec>,
    pub stream_outputs: Vec<PortSpec>,
}

#[derive(Debug, snafu::Snafu)]
pub enum SubgraphMetadataError {
    #[snafu(display("invalid metadata interface: {message}"))]
    Invalid { message: String },
    #[snafu(display("invalid metadata input: {source}"))]
    Input { source: crate::WorkflowInputError },
    #[snafu(display("invalid metadata type: {source}"))]
    TypeDepth { source: crate::TypeDepthError },
}

impl SubgraphMetadata {
    /// Validate frozen declarations without reading environment, stdin, or executing tasks.
    pub fn validate(&self) -> Result<(), SubgraphMetadataError> {
        use snafu::ResultExt;
        for (node, ports) in &self.inputs.inputs {
            for (port, input) in ports {
                snafu::ensure!(
                    !node.is_empty() && !port.is_empty(),
                    InvalidSnafu {
                        message: "input node and port names must not be empty"
                    }
                );
                input.value_type.check_depth().context(TypeDepthSnafu)?;
                input
                    .binding
                    .validate(&crate::output_id(node, port), &input.value_type)
                    .context(InputSnafu)?;
            }
        }
        for ports in [&self.outputs, &self.stream_outputs] {
            let mut names = std::collections::BTreeSet::new();
            for port in ports {
                snafu::ensure!(
                    !port.name.is_empty() && names.insert(&port.name),
                    InvalidSnafu {
                        message: "output names must be nonempty and unique within their channel"
                    }
                );
                port.value_type.check_depth().context(TypeDepthSnafu)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct PreparedSubgraph {
    pub nodes: Vec<NodeIdentity>,
    pub inputs: crate::WorkflowInputSchema,
    pub outputs: Vec<PortSpec>,
    pub stream_outputs: Vec<PortSpec>,
    input_metadata: SubgraphInputMetadata,
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
            input_metadata: Default::default(),
            body: Arc::new(body),
        }
    }

    /// Sets a manually supplied flat schema without inferring independent Flows.
    pub fn with_input_schema(mut self, inputs: crate::WorkflowInputSchema) -> Self {
        self.inputs = inputs;
        self.input_metadata = Default::default();
        self
    }

    /// Captures both the flat runtime schema and its independent-Flow metadata.
    pub fn with_flow_inputs(mut self, flow: &Flow) -> Self {
        self.inputs = flow.input_schema().clone();
        self.input_metadata = SubgraphInputMetadata::from_flow(flow);
        self
    }

    /// Sets matching flat and partitioned views captured before moving a Flow into the body.
    pub fn with_input_metadata(
        mut self,
        inputs: WorkflowInputSchema,
        input_metadata: SubgraphInputMetadata,
    ) -> Self {
        self.inputs = inputs;
        self.input_metadata = input_metadata;
        self
    }

    /// Returns input metadata available to both subgraph factories and executors.
    pub fn input_metadata(&self) -> &SubgraphInputMetadata {
        &self.input_metadata
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
