use crate::{
    ExecutionContext, FlowNode, Inputs, LoopVariableDefinition, NodeExecutionError, NodeMetadata,
    NodePortContract, NodePorts, NodeResult, NodeValue, PortSpec, TaskNode, TypedNodeResult,
    TypedTaskNode, ValueType, encode_typed_result,
};
use snafu::{ResultExt, Snafu, ensure};
use std::collections::BTreeMap;

fn structural_error(message: impl Into<String>) -> NodeExecutionError {
    NodeExecutionError::ExecutionFailed {
        message: message.into(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Snafu)]
pub enum LoopVariableError {
    #[snafu(display("variables must not be empty"))]
    EmptyVariables,
    #[snafu(display("invalid or reserved variable name `{variable}`"))]
    InvalidVariableName { variable: String },
    #[snafu(display("variable `{variable}`: {source}"))]
    VariableType {
        variable: String,
        source: crate::TypeDescriptorError,
    },
    #[snafu(display("duplicate variable `{variable}`"))]
    DuplicateVariable { variable: String },
}

pub fn loop_variable_types(
    variables: &[LoopVariableDefinition],
) -> Result<BTreeMap<String, ValueType>, LoopVariableError> {
    ensure!(!variables.is_empty(), EmptyVariablesSnafu);
    let mut types = BTreeMap::new();
    for variable in variables {
        ensure!(
            !variable.name.trim().is_empty() && variable.name != "index",
            InvalidVariableNameSnafu {
                variable: &variable.name
            }
        );
        let value_type =
            ValueType::parse_descriptor(&variable.value_type).context(VariableTypeSnafu {
                variable: &variable.name,
            })?;
        ensure!(
            types.insert(variable.name.clone(), value_type).is_none(),
            DuplicateVariableSnafu {
                variable: &variable.name
            }
        );
    }
    Ok(types)
}

struct ScopeSourceNode {
    types: BTreeMap<String, ValueType>,
}

#[derive(NodeValue)]
#[value(runtime = "crate")]
struct ScopeIndex {
    index: i64,
}

impl NodePortContract for ScopeSourceNode {
    fn ports(&self) -> NodePorts {
        let mut outputs: Vec<_> = self
            .types
            .iter()
            .map(|(name, value_type)| PortSpec::owned(name, value_type.clone(), true))
            .collect();
        outputs.extend(ScopeIndex::ports());
        NodePorts {
            inputs: Vec::new(),
            outputs,
        }
    }
}

impl TaskNode for ScopeSourceNode {
    fn supports_success_dependencies(&self) -> bool {
        false
    }
    fn execute(
        &self,
        _: Inputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        ctx.scope_values().map(Into::into)
    }
}

pub fn prepared_loop_source_types(types: &BTreeMap<String, ValueType>) -> FlowNode {
    prepared_scope_source(crate::LOOP_SOURCE_ID, types)
}

pub fn prepared_scope_source(id: &str, types: &BTreeMap<String, ValueType>) -> FlowNode {
    FlowNode::new(
        id,
        crate::PreparedNode::new(
            ScopeSourceNode {
                types: types.clone(),
            },
            NodeMetadata::default(),
        )
        .expect("scope source metadata has no port declarations"),
    )
}

struct LoopAssignNode {
    variable: String,
    value_type: ValueType,
}

#[derive(NodeValue)]
#[value(runtime = "crate")]
struct AssignmentOutputs {
    done: bool,
}

impl NodePortContract for LoopAssignNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("value", self.value_type.clone(), true)],
            outputs: AssignmentOutputs::ports(),
        }
    }
}

impl TaskNode for LoopAssignNode {
    fn supports_success_dependencies(&self) -> bool {
        false
    }
    fn execute(
        &self,
        mut inputs: Inputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let value = inputs
            .remove("value")
            .ok_or_else(|| structural_error("missing assignment value"))?;
        ctx.stage_scope_write(&self.variable, value)?;
        encode_typed_result(AssignmentOutputs { done: true }.into())
    }
}

pub fn prepared_loop_assign(id: &str, variable: &str, value_type: ValueType) -> FlowNode {
    FlowNode::new(
        id,
        crate::PreparedNode::new(
            LoopAssignNode {
                variable: variable.to_owned(),
                value_type,
            },
            NodeMetadata::default(),
        )
        .expect("assignment metadata has no port declarations"),
    )
}

struct ExitLoopNode;

#[derive(NodeValue)]
#[value(runtime = "crate")]
struct EmptyValues {}

impl TypedTaskNode for ExitLoopNode {
    fn supports_success_dependencies(&self) -> bool {
        false
    }
    type Input = EmptyValues;
    type Output = EmptyValues;

    fn execute(
        &self,
        _: EmptyValues,
        ctx: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<EmptyValues>, NodeExecutionError> {
        ctx.request_scope_exit()?;
        Ok(EmptyValues {}.into())
    }
}

pub fn prepared_loop_exit(id: &str) -> FlowNode {
    FlowNode::new(
        id,
        crate::PreparedNode::typed_task(ExitLoopNode, NodeMetadata::default())
            .expect("loop exit metadata has no port declarations"),
    )
}
