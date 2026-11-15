use crate::{
    ExecutionContext, FlowNode, Inputs, NodeBuildError, NodeExecutionError, NodeMetadata,
    NodePortContract, NodePorts, NodeResult, Outputs, PortSpec, TaskNode, ValueRef, ValueType,
};
use snafu::{OptionExt, ResultExt};
use std::sync::Arc;

/// Provider-owned state initialization, writable fields, and business validation.
/// Implementations are prepared from the target node's final configuration.
pub trait NodeStateContract: Send + Sync {
    fn initial(&self) -> Outputs;
    fn writable_type(&self, field: &str) -> Option<ValueType>;
    fn validate_write(&self, _field: &str, _value: &ValueRef) -> Result<(), NodeExecutionError> {
        Ok(())
    }
}

struct NodeAssignment {
    source: String,
    field: String,
    value_type: ValueType,
    state: Arc<dyn NodeStateContract>,
}

impl NodePortContract for NodeAssignment {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("value", self.value_type.clone(), true)],
            outputs: vec![PortSpec::new("done", ValueType::Boolean, true)],
        }
    }
}

impl TaskNode for NodeAssignment {
    fn execute(
        &self,
        mut inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let value = inputs
            .remove("value")
            .context(crate::NodeExecutionFailedSnafu {
                message: "missing assignment value",
            })?;
        self.value_type
            .validate_shared(&value)
            .context(crate::node::StateTypeSnafu { field: &self.field })?;
        self.state.validate_write(&self.field, &value)?;
        context.node_state(&self.source, self.state.initial());
        context.stage_node_write(&self.source, &self.field, value)?;
        Ok(Outputs::from([("done".into(), ValueRef::from(true))]).into())
    }
}

pub fn prepared_node_assign(
    id: &str,
    node: &str,
    field: &str,
    state: Arc<dyn NodeStateContract>,
) -> Result<FlowNode, NodeBuildError> {
    let initial = state.initial();
    let value_type = state
        .writable_type(field)
        .filter(|_| initial.contains_key(field))
        .context(crate::node::StateFieldSnafu { field })?;
    value_type
        .validate_shared(&initial[field])
        .context(crate::node::StateInitialSnafu { field })?;
    let prepared = crate::PreparedNode::new(
        NodeAssignment {
            source: format!("%{node}"),
            field: field.to_owned(),
            value_type,
            state,
        },
        NodeMetadata::default(),
    )?;
    Ok(FlowNode::new(id, prepared))
}
