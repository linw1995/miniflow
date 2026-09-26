use crate::{FlowNode, Inputs, NodeExecutionError, Outputs, WorkflowRunError, output_id};
use mf_telemetry::{
    event::{FailurePhase, SkipCause},
    observation::{NodeObservation, RunObservation},
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
pub struct NodeResult {
    pub outputs: Outputs,
    pub skipped: BTreeSet<String>,
}

impl From<Outputs> for NodeResult {
    fn from(outputs: Outputs) -> Self {
        Self {
            outputs,
            skipped: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ContextValue<'a> {
    Value(&'a Value),
    Skipped,
}

/// Completed output values for one run. Nodes receive an immutable reference.
#[derive(Debug, Default)]
pub struct ExecutionContext {
    // A present None is an explicit skip; an absent key is a missing output.
    outputs: BTreeMap<String, Option<Value>>,
    observation: Option<RunObservation>,
}

impl ExecutionContext {
    /// Runs a synchronous execution scope without changing its result or installing providers.
    pub fn run<T, E: std::fmt::Display>(
        observation: Option<RunObservation>,
        execute: impl FnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut state = Self {
            observation,
            outputs: BTreeMap::new(),
        };
        let _context = state.observation.as_ref().map(RunObservation::enter);
        let result = execute(&mut state);
        if let Some(run) = state.observation.as_mut() {
            run.finish(
                result
                    .as_ref()
                    .err()
                    .map(|error| error as &dyn std::fmt::Display),
            );
        }
        result
    }

    pub fn prepare_node(
        &mut self,
        registry: &crate::NodeRegistry,
        id: &str,
        kind: &str,
        config: &str,
    ) -> Result<FlowNode, WorkflowRunError> {
        let result = crate::instantiate_node_with_metadata(registry, id, kind, config);
        if let Err(error) = &result {
            self.preparation_failed(id, error);
        }
        result
    }

    pub fn preparation_failed(&mut self, id: &str, error: &dyn std::fmt::Display) {
        if let Some(run) = self.observation.as_mut() {
            run.preparation_failed(id, error.to_string());
        }
    }

    pub fn select_output(
        &mut self,
        name: &str,
        node: &str,
        port: &str,
        optional: bool,
    ) -> Result<Option<Value>, WorkflowRunError> {
        let result = select_context_output(self, name, node, port, optional);
        if let (Err(error), Some(run)) = (&result, self.observation.as_mut()) {
            run.output_selection_failed(error.to_string());
        }
        result
    }

    pub fn output(&self, id: &str) -> Result<ContextValue<'_>, NodeExecutionError> {
        match self.outputs.get(id) {
            Some(Some(value)) => Ok(ContextValue::Value(value)),
            Some(None) => Ok(ContextValue::Skipped),
            None => Err(NodeExecutionError::ExecutionFailed {
                message: format!("missing context output `{id}`"),
            }),
        }
    }

    fn publish(
        &mut self,
        node: &FlowNode,
        result: Option<NodeResult>,
    ) -> Result<(), WorkflowRunError> {
        let id = node.definition_id.as_str();
        if let Some(result) = &result {
            for name in &result.skipped {
                if result.outputs.contains_key(name) {
                    return Err(state_error(
                        id,
                        format!("output `{name}` is both produced and skipped"),
                    ));
                }
                if !node
                    .ports
                    .outputs
                    .iter()
                    .any(|port| port.name == *name && !port.required)
                {
                    return Err(state_error(
                        id,
                        format!("cannot explicitly skip unknown or required output `{name}`"),
                    ));
                }
            }
            for name in result.outputs.keys() {
                if !node.ports.outputs.iter().any(|port| port.name == *name) {
                    return Err(state_error(
                        id,
                        format!("produced undeclared output `{name}`"),
                    ));
                }
            }
        }
        // Validate the complete result before making any values visible.
        match result {
            Some(result) => {
                self.outputs.extend(
                    result
                        .outputs
                        .into_iter()
                        .map(|(port, value)| (output_id(id, &port), Some(value))),
                );
                self.outputs.extend(
                    result
                        .skipped
                        .into_iter()
                        .map(|port| (output_id(id, &port), None)),
                );
            }
            None => self.outputs.extend(
                node.ports
                    .outputs
                    .iter()
                    .map(|port| (output_id(id, &port.name), None)),
            ),
        }
        Ok(())
    }
}

fn state_error(id: &str, message: impl Into<String>) -> WorkflowRunError {
    WorkflowRunError::Context {
        definition_id: id.into(),
        message: message.into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExecutionDependency<'a> {
    pub input: Option<&'a str>,
    pub source_node: &'a str,
    pub source_output: &'a str,
}

/// Executes one step of a validated plan, in its topological order.
pub fn execute_node_in_context(
    node: &FlowNode,
    dependencies: &[ExecutionDependency<'_>],
    ctx: &mut ExecutionContext,
) -> Result<(), WorkflowRunError> {
    let id = node.definition_id.as_str();
    let mut step = ctx.observation.as_mut().and_then(|run| run.begin_node(id));
    let _context = step.as_ref().map(NodeObservation::enter);
    let mut dependencies = dependencies.to_vec();
    dependencies.sort();
    let mut inputs = Inputs::new();
    let mut skipped = false;
    let mut causes = BTreeSet::new();
    for dependency in dependencies {
        let value = ctx
            .output(&output_id(dependency.source_node, dependency.source_output))
            .map_err(|error| {
                state_error(
                    id,
                    format!(
                        "dependency {}: {error}",
                        dependency.input.unwrap_or("<control>")
                    ),
                )
            });
        let value = match value {
            Ok(value) => value,
            Err(error) => {
                if let (Some(run), Some(step)) = (ctx.observation.as_mut(), step) {
                    run.node_failed(step, FailurePhase::Dependency, error.to_string());
                }
                return Err(error);
            }
        };
        match value {
            ContextValue::Value(value) => {
                if let Some(input) = dependency.input {
                    inputs.insert(input.to_owned(), value.clone());
                }
            }
            ContextValue::Skipped => {
                skipped = true;
                if step.is_some() {
                    causes.insert(SkipCause {
                        source_node: dependency.source_node.into(),
                        source_output: dependency.source_output.into(),
                    });
                }
            }
        }
    }
    let result = if skipped {
        None
    } else {
        if let (Some(run), Some(step)) = (ctx.observation.as_mut(), step.as_mut()) {
            run.node_started(step);
        }
        let result = node
            .node
            .execute_with_context(inputs, ctx)
            .map_err(|source| WorkflowRunError::NodeExecution {
                definition_id: node.definition_id.clone(),
                source,
            });
        match result {
            Ok(result) => Some(result),
            Err(error) => {
                if let (Some(run), Some(step)) = (ctx.observation.as_mut(), step) {
                    run.node_failed(step, FailurePhase::Execution, error.to_string());
                }
                return Err(error);
            }
        }
    };
    let mut produced_ports = Vec::new();
    let mut skipped_ports = Vec::new();
    if step.is_some() {
        if let Some(result) = &result {
            produced_ports.extend(result.outputs.keys().cloned());
            skipped_ports.extend(result.skipped.iter().cloned());
        } else {
            skipped_ports.extend(node.ports.outputs.iter().map(|port| port.name.to_string()));
            skipped_ports.sort();
        }
    }
    let result = ctx.publish(node, result);
    if let (Some(run), Some(step)) = (ctx.observation.as_mut(), step) {
        match &result {
            Err(error) => run.node_failed(step, FailurePhase::Publication, error.to_string()),
            Ok(()) if skipped => {
                run.node_skipped(step, causes.into_iter().collect(), skipped_ports)
            }
            Ok(()) => run.node_succeeded(step, produced_ports, skipped_ports),
        }
    }
    result
}

/// Reads a selection without instrumentation; observed executors use `ExecutionContext::select_output`.
pub fn select_context_output(
    ctx: &ExecutionContext,
    name: &str,
    node: &str,
    port: &str,
    optional: bool,
) -> Result<Option<Value>, WorkflowRunError> {
    match ctx
        .output(&output_id(node, port))
        .map_err(|error| state_error(node, format!("workflow output `{name}`: {error}")))?
    {
        ContextValue::Value(value) => Ok(Some(value.clone())),
        ContextValue::Skipped if optional => Ok(None),
        ContextValue::Skipped => Err(state_error(
            node,
            format!(
                "workflow output `{name}` requires skipped output `{}`",
                output_id(node, port)
            ),
        )),
    }
}
