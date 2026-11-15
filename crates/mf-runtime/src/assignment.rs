use crate::{
    ExecutionContext, FlowNode, Inputs, NodeBuildError, NodeExecutionError, NodeMetadata,
    NodePortContract, NodePorts, NodeResult, Outputs, PortSpec, StateTarget, TaskNode, ValueRef,
    ValueType,
};
use snafu::{OptionExt, ResultExt, ensure};
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

struct AssignmentNode {
    target: StateTarget,
    value_type: ValueType,
    state: Option<(String, Arc<dyn NodeStateContract>)>,
}

impl NodePortContract for AssignmentNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("value", self.value_type.clone(), true)],
            outputs: vec![PortSpec::new("done", ValueType::Boolean, true)],
        }
    }
}

impl TaskNode for AssignmentNode {
    fn supports_success_dependencies(&self) -> bool {
        matches!(self.target, StateTarget::Node { .. })
    }

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
        if let Some((source, state)) = &self.state {
            self.value_type
                .validate_shared(&value)
                .context(crate::node::StateTypeSnafu {
                    field: self.target.field(),
                })?;
            state.validate_write(self.target.field(), &value)?;
            context.node_state(source, state.initial());
        }
        context.stage_write(self.target.clone(), value)?;
        Ok(Outputs::from([("done".into(), ValueRef::from(true))]).into())
    }
}

pub fn prepared_assign(
    id: &str,
    target: StateTarget,
    value_type: ValueType,
    state: Option<Arc<dyn NodeStateContract>>,
) -> Result<FlowNode, NodeBuildError> {
    ensure!(
        matches!(target, StateTarget::Node { .. }) == state.is_some(),
        crate::node::StateContractSnafu
    );
    if let Some(state) = &state {
        let field = target.field();
        let initial = state.initial();
        let declared = state
            .writable_type(field)
            .filter(|_| initial.contains_key(field))
            .context(crate::node::StateFieldSnafu { field })?;
        ensure!(declared == value_type, crate::node::StateContractSnafu);
        value_type
            .validate_shared(&initial[field])
            .context(crate::node::StateInitialSnafu { field })?;
    }
    let state = state.map(|state| {
        let StateTarget::Node { node, .. } = &target else {
            unreachable!("provider state requires a node target")
        };
        (format!("%{node}"), state)
    });
    let prepared = crate::PreparedNode::new(
        AssignmentNode {
            target,
            value_type,
            state,
        },
        NodeMetadata::default(),
    )?;
    Ok(FlowNode::new(id, prepared))
}
