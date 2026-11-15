use crate::{
    ExecutionContext, FlowNode, Inputs, LoopVariableDefinition, NodeExecutionError, NodeMetadata,
    NodePortContract, NodePorts, NodeResult, NodeValue, PortSpec, TaskNode, TypedNodeResult,
    TypedTaskNode, ValueType,
};
use snafu::{ResultExt, Snafu, ensure};
use std::collections::BTreeMap;

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
    root: bool,
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
        if !self.root {
            outputs.extend(ScopeIndex::ports());
        }
        NodePorts {
            inputs: Vec::new(),
            outputs,
        }
    }
}

impl TaskNode for ScopeSourceNode {
    fn operation(&self) -> Option<crate::NodeOperation> {
        Some(if self.root {
            crate::NodeOperation::RootInput
        } else {
            crate::NodeOperation::Input
        })
    }

    fn supports_success_dependencies(&self) -> bool {
        false
    }
    fn execute(
        &self,
        _: Inputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        if self.root {
            ctx.root_values().map(Into::into)
        } else {
            ctx.scope_values().map(Into::into)
        }
    }
}

pub fn prepared_loop_source_types(types: &BTreeMap<String, ValueType>) -> FlowNode {
    prepared_scope_source(crate::LOOP_SOURCE_ID, types)
}

pub fn prepared_scope_source(id: &str, types: &BTreeMap<String, ValueType>) -> FlowNode {
    prepared_source(id, types, false)
}

pub fn prepared_root_source(id: &str, types: &BTreeMap<String, ValueType>) -> FlowNode {
    prepared_source(id, types, true)
}

fn prepared_source(id: &str, types: &BTreeMap<String, ValueType>, root: bool) -> FlowNode {
    FlowNode::new(
        id,
        crate::PreparedNode::new(
            ScopeSourceNode {
                types: types.clone(),
                root,
            },
            NodeMetadata::default(),
        )
        .expect("scope source metadata has no port declarations"),
    )
}

struct ExitLoopNode;

#[derive(NodeValue)]
#[value(runtime = "crate")]
struct EmptyValues {}

impl TypedTaskNode for ExitLoopNode {
    fn operation(&self) -> Option<crate::NodeOperation> {
        Some(crate::NodeOperation::Exit)
    }

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
