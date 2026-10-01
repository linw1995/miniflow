use crate::scope_observation::LoopScopeObserver;
use mf_runtime::{
    ExecutionContext, ExecutionScope, Inputs, LoopComparisonOperator, LoopConditionDefinition,
    LoopVariableDefinition, Node, NodeBuildError, NodeExecutionError, NodePorts, NodeRegistration,
    NodeResult, Outputs, PortSpec, PreparedSubgraph, ValueType, compare_json_numbers,
    deserialize_config, loop_variable_types,
};
use mf_telemetry::{
    Count,
    event::{LoopStopReason, LoopSummary},
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::{cmp::Ordering, collections::BTreeMap, ops::ControlFlow};

pub const KIND: &str = mf_runtime::LOOP_KIND;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}

struct LoopDeclaration;

impl Node for LoopDeclaration {
    fn with_subgraph(
        self: Box<Self>,
        id: &str,
        options: Value,
        body: PreparedSubgraph,
    ) -> Result<Box<dyn Node>, NodeBuildError> {
        let config: ExecutionConfig = deserialize_config(options)?;
        if !(1..=mf_runtime::MAX_LOOP_ITERATIONS).contains(&config.max_iterations) {
            return Err(NodeBuildError::InvalidSubgraph {
                message: "max_iterations must be in 1..=1000".into(),
            });
        }
        let types = loop_variable_types(&config.variables)
            .map_err(|message| NodeBuildError::InvalidSubgraph { message })?;
        let ports: Vec<_> = types
            .iter()
            .map(|(name, value_type)| PortSpec::owned(name, value_type.clone(), true))
            .collect();
        Ok(Box::new(LoopNode {
            id: id.into(),
            max_iterations: config.max_iterations,
            until: config.until,
            types,
            body,
            ports: NodePorts {
                inputs: ports.clone(),
                outputs: ports,
            },
            observer: Arc::new(LoopScopeObserver),
        }))
    }

    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(NodeExecutionError::ExecutionFailed {
            message: "Loop requires a compiled body".into(),
        })
    }
}

fn factory(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    let _: Config = deserialize_config(config)?;
    Ok(Box::new(LoopDeclaration))
}

// Typed ports are supplied when execution settings and the prepared body are bound.
inventory::submit! {
    NodeRegistration {
        kind: KIND,
        inputs: &[],
        outputs: &[],
        factory,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::NodeRegistry;
    use serde_json::json;

    #[test]
    fn registers_loop_declaration_and_requires_an_empty_config() {
        let registration = NodeRegistry::from_inventory().unwrap().get(KIND).unwrap();
        assert!(
            registration
                .instantiate(json!({"unexpected": true}))
                .is_err()
        );
        assert!(registration.instantiate(json!({})).is_ok());
    }
}

pub fn run_loop<T>(
    steps: impl IntoIterator<Item = T>,
    mut run: impl FnMut(T) -> Result<ControlFlow<()>, NodeExecutionError>,
) -> Result<(), NodeExecutionError> {
    for step in steps {
        if run(step)?.is_break() {
            break;
        }
    }
    Ok(())
}

fn structural_error(message: impl Into<String>) -> NodeExecutionError {
    NodeExecutionError::ExecutionFailed {
        message: message.into(),
    }
}

// Older generated sources include a body field; it is not needed to configure execution.
#[derive(Deserialize)]
struct ExecutionConfig {
    max_iterations: u16,
    variables: Vec<LoopVariableDefinition>,
    #[serde(default)]
    until: Option<LoopConditionDefinition>,
}

struct LoopNode {
    id: String,
    max_iterations: u16,
    until: Option<LoopConditionDefinition>,
    types: BTreeMap<String, ValueType>,
    ports: NodePorts,
    body: PreparedSubgraph,
    observer: Arc<LoopScopeObserver>,
}

impl Node for LoopNode {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(structural_error(
            "Loop requires a workflow execution context",
        ))
    }

    fn ports(&self) -> Option<NodePorts> {
        Some(self.ports.clone())
    }

    fn execute_with_context_mut(
        &self,
        inputs: Inputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let mut variables = inputs;
        let mut pass_count = 0;
        let mut reason = LoopStopReason::Maximum;
        run_loop(0..usize::from(self.max_iterations), |index| {
            let scope = ExecutionScope::new(
                &self.id,
                mf_runtime::LOOP_SOURCE_ID,
                index,
                std::mem::take(&mut variables),
                self.types.clone(),
            )?
            .with_observer(self.observer.clone());
            let (_, updated, exited) = ctx
                .run_scope(scope, |state| self.body.execute_in_context(state))
                .map_err(|error| {
                    structural_error(format!("Loop `{}` pass {index}: {error}", self.id))
                })?;
            variables = updated;
            pass_count = index + 1;
            if exited {
                reason = LoopStopReason::Exit;
                return Ok(ControlFlow::Break(()));
            }
            if let Some(condition) = &self.until
                && condition_matches(condition, &variables)?
            {
                reason = LoopStopReason::Condition;
                return Ok(ControlFlow::Break(()));
            }
            Ok(ControlFlow::Continue(()))
        })?;
        Ok(NodeResult {
            outputs: variables,
            loop_summary: Some(LoopSummary {
                pass_count: Count::try_from(pass_count as i64).expect("Loop pass count is bounded"),
                reason,
            }),
            ..NodeResult::default()
        })
    }
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
