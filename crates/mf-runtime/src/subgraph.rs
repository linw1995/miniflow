use crate::{ExecutionContext, FlowOutputs, NodeIdentity, PortSpec, WorkflowRunError};

type SubgraphBody =
    dyn Fn(&mut ExecutionContext) -> Result<FlowOutputs, WorkflowRunError> + Send + Sync + 'static;

pub struct PreparedSubgraph {
    pub nodes: Vec<NodeIdentity>,
    pub inputs: crate::WorkflowInputSchema,
    pub outputs: Vec<PortSpec>,
    body: Box<SubgraphBody>,
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
        Self {
            nodes,
            inputs: Default::default(),
            outputs,
            body: Box::new(body),
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
        (self.body)(state)
    }
}
