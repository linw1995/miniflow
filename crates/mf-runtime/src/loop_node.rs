use crate::{
    ExecutionContext, FlowNode, Inputs, LoopVariableDefinition, Node, NodeExecutionError,
    NodePorts, NodeResult, Outputs, PortSpec, ValueType, WorkflowRunError,
};
use serde_json::Value;
use std::collections::BTreeMap;

fn structural_error(message: impl Into<String>) -> NodeExecutionError {
    NodeExecutionError::ExecutionFailed {
        message: message.into(),
    }
}

pub fn loop_variable_types(
    variables: &[LoopVariableDefinition],
) -> Result<BTreeMap<String, ValueType>, String> {
    if variables.is_empty() {
        return Err("variables must not be empty".into());
    }
    let mut types = BTreeMap::new();
    for variable in variables {
        if variable.name.trim().is_empty() || variable.name == "index" {
            return Err(format!(
                "invalid or reserved variable name `{}`",
                variable.name
            ));
        }
        let value_type = ValueType::parse_descriptor(&variable.value_type)
            .map_err(|error| format!("variable `{}`: {error}", variable.name))?;
        if types.insert(variable.name.clone(), value_type).is_some() {
            return Err(format!("duplicate variable `{}`", variable.name));
        }
    }
    Ok(types)
}

struct ScopeSourceNode;

impl Node for ScopeSourceNode {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(structural_error(
            "scope source requires an execution context",
        ))
    }

    fn execute_with_context_mut(
        &self,
        _: Inputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        ctx.scope_values().map(Into::into)
    }
}

pub fn prepared_loop_source_from_json(variables_json: &str) -> Result<FlowNode, WorkflowRunError> {
    let variables: Vec<LoopVariableDefinition> =
        serde_json::from_str(variables_json).map_err(|source| {
            WorkflowRunError::InvalidEmbeddedConfig {
                definition_id: crate::LOOP_SOURCE_ID.into(),
                source,
            }
        })?;
    let types = loop_variable_types(&variables).map_err(|message| WorkflowRunError::Context {
        definition_id: crate::LOOP_SOURCE_ID.into(),
        message,
    })?;
    Ok(prepared_loop_source_types(&types))
}

pub fn prepared_loop_source_types(types: &BTreeMap<String, ValueType>) -> FlowNode {
    prepared_scope_source(crate::LOOP_SOURCE_ID, types)
}

pub fn prepared_scope_source(id: &str, types: &BTreeMap<String, ValueType>) -> FlowNode {
    let mut outputs: Vec<_> = types
        .iter()
        .map(|(name, value_type)| PortSpec::owned(name, value_type.clone(), true))
        .collect();
    outputs.push(PortSpec::owned("index", ValueType::Int64, true));
    let ports = NodePorts {
        inputs: Vec::new(),
        outputs,
    };
    FlowNode::new(id, Box::new(ScopeSourceNode), ports)
}

pub fn prepared_scope_source_from_json(
    id: &str,
    types: &str,
) -> Result<FlowNode, WorkflowRunError> {
    let descriptors: BTreeMap<String, Value> =
        serde_json::from_str(types).map_err(|source| WorkflowRunError::InvalidEmbeddedConfig {
            definition_id: id.into(),
            source,
        })?;
    let types = descriptors
        .into_iter()
        .map(|(name, value)| {
            ValueType::parse_descriptor(&value).map(|value_type| (name, value_type))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(|message| WorkflowRunError::Context {
            definition_id: id.into(),
            message,
        })?;
    Ok(prepared_scope_source(id, &types))
}

struct LoopAssignNode {
    variable: String,
}

impl Node for LoopAssignNode {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(structural_error("Loop assignment requires a Loop frame"))
    }

    fn execute_with_context_mut(
        &self,
        mut inputs: Inputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let value = inputs
            .remove("value")
            .ok_or_else(|| structural_error("missing assignment value"))?;
        ctx.stage_scope_write(&self.variable, value)?;
        Ok(Outputs::from([("done".into(), Value::Bool(true))]).into())
    }
}

pub fn prepared_loop_assign(id: &str, variable: &str, value_type: ValueType) -> FlowNode {
    let ports = NodePorts {
        inputs: vec![PortSpec::owned("value", value_type, true)],
        outputs: vec![PortSpec::owned("done", ValueType::Boolean, true)],
    };
    FlowNode::new(
        id,
        Box::new(LoopAssignNode {
            variable: variable.to_owned(),
        }),
        ports,
    )
}

pub fn prepared_loop_assign_from_json(
    id: &str,
    variable: &str,
    type_json: &str,
) -> Result<FlowNode, WorkflowRunError> {
    let descriptor: Value = serde_json::from_str(type_json).map_err(|source| {
        WorkflowRunError::InvalidEmbeddedConfig {
            definition_id: id.into(),
            source,
        }
    })?;
    let value_type =
        ValueType::parse_descriptor(&descriptor).map_err(|message| WorkflowRunError::Context {
            definition_id: id.into(),
            message,
        })?;
    Ok(prepared_loop_assign(id, variable, value_type))
}

struct ExitLoopNode;

impl Node for ExitLoopNode {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(structural_error("Loop exit requires a Loop frame"))
    }

    fn execute_with_context_mut(
        &self,
        _: Inputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        ctx.request_scope_exit()?;
        Ok(Outputs::new().into())
    }
}

pub fn prepared_loop_exit(id: &str) -> FlowNode {
    FlowNode::new(id, Box::new(ExitLoopNode), NodePorts::default())
}
