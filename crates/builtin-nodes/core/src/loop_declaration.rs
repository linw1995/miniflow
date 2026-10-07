use mf_runtime::{
    ExecutionContext, ExecutionScope, Inputs, LoopComparisonOperator, LoopConditionDefinition,
    LoopVariableDefinition, NodeBuildError, NodeExecutionError, NodePorts, NodeRegistration,
    NodeResult, Outputs, PortSpec, PreparedSubgraph, TaskNode, ValueType, compare_json_numbers,
    deserialize_config, loop_variable_types,
};
use mf_telemetry::{
    Count,
    event::{LoopPassOutcome, LoopStopReason, LoopSummary},
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{ResultExt, Snafu, ensure};
use std::{cmp::Ordering, collections::BTreeMap};

pub const KIND: &str = mf_runtime::LOOP_KIND;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}

fn factory(
    id: &str,
    config: Value,
    options: Value,
    body: PreparedSubgraph,
) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let _: Config = deserialize_config(config)?;
    let config: ExecutionConfig = deserialize_config(options)?;
    ensure!(
        (1..=mf_runtime::MAX_LOOP_ITERATIONS).contains(&config.max_iterations),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "max_iterations must be in 1..=1000",
        }
    );
    let types = loop_variable_types(&config.variables)
        .map_err(|message| mf_runtime::NodeInvalidSubgraphSnafu { message }.build())?;
    let ports: Vec<_> = types
        .iter()
        .map(|(name, value_type)| PortSpec::owned(name, value_type.clone(), true))
        .collect();
    Ok(mf_runtime::PreparedNode::new(
        LoopNode {
            id: id.into(),
            max_iterations: config.max_iterations,
            until: config.until,
            types,
            body,
        },
        NodePorts {
            inputs: ports.clone(),
            outputs: ports,
        },
    ))
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Subgraph(factory) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::NodeRegistry;
    use serde_json::json;

    #[test]
    fn registers_loop_factory_and_requires_a_body_and_empty_config() {
        let registration = NodeRegistry::from_inventory().unwrap().get(KIND).unwrap();
        assert!(
            registration
                .instantiate(json!({"unexpected": true}))
                .is_err()
        );
        assert!(registration.instantiate(json!({})).is_err());
        for (config, valid) in [(json!({}), true), (json!({"unexpected": true}), false)] {
            let prepared = registration.instantiate_subgraph(
                "repeat",
                config,
                json!({"max_iterations":1, "variables":[{"name":"x", "type":"int"}]}),
                PreparedSubgraph::new(Vec::new(), Vec::new(), |_| Ok(Outputs::new())),
            );
            assert_eq!(prepared.is_ok(), valid);
        }
    }
}

fn structural_error(message: impl Into<String>) -> NodeExecutionError {
    mf_runtime::NodeExecutionFailedSnafu {
        message: message.into(),
    }
    .build()
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
    body: PreparedSubgraph,
}

#[derive(Debug, Snafu)]
#[snafu(display("Loop `{node}` pass {index}: {source}"))]
struct LoopPassFailure {
    node: String,
    index: usize,
    source: mf_runtime::WorkflowRunError,
}

impl From<LoopPassFailure> for NodeExecutionError {
    fn from(source: LoopPassFailure) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

impl TaskNode for LoopNode {
    fn execute(
        &self,
        inputs: Inputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let mut variables = inputs;
        let mut pass_count = 0;
        let mut reason = LoopStopReason::Maximum;
        for index in 0..usize::from(self.max_iterations) {
            let scope = ExecutionScope::new(
                &self.id,
                mf_runtime::LOOP_SOURCE_ID,
                index,
                std::mem::take(&mut variables),
                self.types.clone(),
            )?;
            let (_, updated, exited) = ctx
                .run_scope(scope, |state| {
                    let path = state.scope_path();
                    if let Some(run) = state.observation_mut().filter(|run| run.supports_loops()) {
                        run.loop_pass_started(path.clone());
                    }
                    let result = self.body.execute_in_context(state);
                    let outcome = if result.is_err() {
                        LoopPassOutcome::Failed
                    } else if state.scope_exit_requested() {
                        LoopPassOutcome::Exit
                    } else {
                        LoopPassOutcome::Completed
                    };
                    let visited = Count::try_from(state.scope_visited_steps() as i64)
                        .expect("scope budget bounds visits");
                    if let Some(run) = state.observation_mut().filter(|run| run.supports_loops()) {
                        run.loop_pass_finished(path, visited, outcome);
                    }
                    result
                })
                .context(LoopPassFailureSnafu {
                    node: self.id.clone(),
                    index,
                })?;
            variables = updated;
            pass_count = index + 1;
            if exited {
                reason = LoopStopReason::Exit;
                break;
            }
            if let Some(condition) = &self.until
                && condition_matches(condition, &variables)?
            {
                reason = LoopStopReason::Condition;
                break;
            }
        }
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
    variables: &Outputs,
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
    let order = match (actual.as_number(), expected.as_number()) {
        (Some(left), Some(right)) => Some(compare_json_numbers(left, right)),
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
