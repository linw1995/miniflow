use crate::{
    DefinitionId, ExecutionContext, FlowOutputs, LoopBodyDefinition, NodeDefinition, NodeIdentity,
    PortSpec, ValueType, WorkflowDefinition, WorkflowOutputDefinition, WorkflowRunError,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub const SCOPE_INPUT_KIND: &str = "scope.input";

pub struct SubgraphDefinition {
    pub body: LoopBodyDefinition,
    pub body_pointer: String,
    pub source_id: DefinitionId,
    pub inputs: BTreeMap<String, ValueType>,
    pub outputs: Vec<WorkflowOutputDefinition>,
    pub options: Value,
    pub allow_state: bool,
}

impl SubgraphDefinition {
    pub fn workflow(&self, parent: &WorkflowDefinition) -> WorkflowDefinition {
        let mut nodes = vec![NodeDefinition {
            id: self.source_id.clone(),
            kind: SCOPE_INPUT_KIND.into(),
            config: Value::Object(Default::default()),
            loop_definition: None,
        }];
        nodes.extend(self.body.nodes.iter().cloned());
        WorkflowDefinition {
            version: parent.version,
            dependencies: parent.dependencies.clone(),
            nodes,
            edges: self.body.edges.clone(),
            control_edges: self.body.control_edges.clone(),
            outputs: self.outputs.clone(),
        }
    }
}

type SubgraphBody =
    dyn Fn(&mut ExecutionContext) -> Result<FlowOutputs, WorkflowRunError> + Send + Sync + 'static;

pub struct PreparedSubgraph {
    pub nodes: Vec<NodeIdentity>,
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
            outputs,
            body: Box::new(body),
        }
    }

    pub fn execute_in_context(
        &self,
        state: &mut ExecutionContext,
    ) -> Result<FlowOutputs, WorkflowRunError> {
        (self.body)(state)
    }
}
