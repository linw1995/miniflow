use crate::{
    ExecutionContext, FlowNode, Inputs, LoopComparisonOperator, LoopConditionDefinition,
    LoopDefinition, LoopVariableDefinition, Node, NodeExecutionError, NodePorts, NodeResult,
    Outputs, PortSpec, ValueType, WorkflowRunError, compare_json_numbers,
};
use mf_telemetry::event::LoopStopReason;
use serde_json::Value;
use std::{cmp::Ordering, collections::BTreeMap};

fn structural_error(message: impl Into<String>) -> NodeExecutionError {
    NodeExecutionError::ExecutionFailed {
        message: message.into(),
    }
}

fn variable_types(
    variables: &[LoopVariableDefinition],
) -> Result<BTreeMap<String, ValueType>, String> {
    let mut types = BTreeMap::new();
    for variable in variables {
        let value_type = ValueType::parse_descriptor(&variable.value_type)
            .map_err(|error| format!("variable `{}`: {error}", variable.name))?;
        if types.insert(variable.name.clone(), value_type).is_some() {
            return Err(format!("duplicate Loop variable `{}`", variable.name));
        }
    }
    Ok(types)
}

fn condition_matches(
    condition: &LoopConditionDefinition,
    variables: &BTreeMap<String, Value>,
) -> Result<bool, NodeExecutionError> {
    let actual = variables.get(&condition.variable).ok_or_else(|| {
        structural_error(format!("missing Loop variable `{}`", condition.variable))
    })?;
    let expected = &condition.value;
    if actual.is_array() || actual.is_object() {
        return Err(structural_error(format!(
            "Loop condition variable `{}` must be scalar",
            condition.variable
        )));
    }
    let order = match (actual, expected) {
        (Value::Number(left), Value::Number(right)) => Some(compare_json_numbers(left, right)),
        _ => None,
    };
    let result = match condition.operator {
        LoopComparisonOperator::Eq => {
            order.map_or(actual == expected, |order| order == Ordering::Equal)
        }
        LoopComparisonOperator::Ne => {
            order.map_or(actual != expected, |order| order != Ordering::Equal)
        }
        LoopComparisonOperator::Gt => order == Some(Ordering::Greater),
        LoopComparisonOperator::Gte => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
        LoopComparisonOperator::Lt => order == Some(Ordering::Less),
        LoopComparisonOperator::Lte => matches!(order, Some(Ordering::Less | Ordering::Equal)),
    };
    if matches!(
        condition.operator,
        LoopComparisonOperator::Gt
            | LoopComparisonOperator::Gte
            | LoopComparisonOperator::Lt
            | LoopComparisonOperator::Lte
    ) && order.is_none()
    {
        return Err(structural_error(format!(
            "Loop condition variable `{}` requires numeric operands",
            condition.variable
        )));
    }
    Ok(result)
}

struct LoopNode<F> {
    id: String,
    definition: LoopDefinition,
    types: BTreeMap<String, ValueType>,
    body: F,
}

impl<F> Node for LoopNode<F>
where
    F: Fn(&mut ExecutionContext) -> Result<(), WorkflowRunError> + Send + Sync,
{
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(structural_error(
            "Loop requires a workflow execution context",
        ))
    }

    fn execute_with_context_mut(
        &self,
        inputs: Inputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let mut variables = inputs;
        let mut pass_count = 0;
        let mut reason = LoopStopReason::Maximum;
        for index in 0..usize::from(self.definition.max_iterations) {
            let (updated, exited) = ctx
                .run_loop_frame(&self.id, variables, self.types.clone(), index, |ctx| {
                    (self.body)(ctx)
                })
                .map_err(|error| {
                    structural_error(format!("Loop `{}` pass {index}: {error}", self.id))
                })?;
            variables = updated;
            pass_count = index + 1;
            if exited {
                reason = LoopStopReason::Exit;
                break;
            }
            if let Some(condition) = &self.definition.until
                && condition_matches(condition, &variables)?
            {
                reason = LoopStopReason::Condition;
                break;
            }
        }
        ctx.set_loop_summary(pass_count, reason);
        Ok(variables.into())
    }
}

pub fn prepared_loop_node<F>(
    id: &str,
    definition: LoopDefinition,
    body: F,
) -> Result<FlowNode, WorkflowRunError>
where
    F: Fn(&mut ExecutionContext) -> Result<(), WorkflowRunError> + Send + Sync + 'static,
{
    let types =
        variable_types(&definition.variables).map_err(|message| WorkflowRunError::Context {
            definition_id: id.into(),
            message,
        })?;
    let variables: Vec<_> = types
        .iter()
        .map(|(name, value_type)| PortSpec::owned(name, value_type.clone(), true))
        .collect();
    let ports = NodePorts {
        inputs: variables.clone(),
        outputs: variables,
    };
    Ok(FlowNode::new(
        id,
        Box::new(LoopNode {
            id: id.to_owned(),
            definition,
            types,
            body,
        }),
        ports,
    ))
}

pub fn prepared_loop_node_from_json<F>(
    id: &str,
    definition_json: &str,
    body: F,
) -> Result<FlowNode, WorkflowRunError>
where
    F: Fn(&mut ExecutionContext) -> Result<(), WorkflowRunError> + Send + Sync + 'static,
{
    let definition: LoopDefinition = serde_json::from_str(definition_json).map_err(|source| {
        WorkflowRunError::InvalidEmbeddedConfig {
            definition_id: id.into(),
            source,
        }
    })?;
    prepared_loop_node(id, definition, body)
}

struct LoopSourceNode;

impl Node for LoopSourceNode {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(structural_error("Loop source requires a Loop frame"))
    }

    fn execute_with_context_mut(
        &self,
        _: Inputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        ctx.loop_source_values().map(Into::into)
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
    let types = variable_types(&variables).map_err(|message| WorkflowRunError::Context {
        definition_id: crate::LOOP_SOURCE_ID.into(),
        message,
    })?;
    Ok(prepared_loop_source_types(&types))
}

pub fn prepared_loop_source_types(types: &BTreeMap<String, ValueType>) -> FlowNode {
    let mut outputs: Vec<_> = types
        .iter()
        .map(|(name, value_type)| PortSpec::owned(name, value_type.clone(), true))
        .collect();
    outputs.push(PortSpec::owned("index", ValueType::Int64, true));
    let ports = NodePorts {
        inputs: Vec::new(),
        outputs,
    };
    FlowNode::new(crate::LOOP_SOURCE_ID, Box::new(LoopSourceNode), ports)
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
        ctx.stage_loop_write(&self.variable, value)?;
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
        ctx.request_loop_exit()?;
        Ok(Outputs::new().into())
    }
}

pub fn prepared_loop_exit(id: &str) -> FlowNode {
    FlowNode::new(id, Box::new(ExitLoopNode), NodePorts::default())
}
