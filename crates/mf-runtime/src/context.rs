use crate::{FlowNode, Inputs, NodeExecutionError, Outputs, WorkflowRunError, output_id};
use mf_telemetry::{
    event::{FailurePhase, NodeIdentity, SkipCause},
    observation::{
        BodyNodeObservation, BodyObservation, IterationObservation, NodeObservation, RunObservation,
    },
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
    body_observation: Option<BodyObservation>,
}

impl ExecutionContext {
    pub fn for_iteration(item: Value, index: usize) -> Result<Self, NodeExecutionError> {
        Self::for_iteration_with_observation(item, index, None)
    }

    pub fn for_iteration_with_observation(
        item: Value,
        index: usize,
        body_observation: Option<BodyObservation>,
    ) -> Result<Self, NodeExecutionError> {
        let index = i64::try_from(index).map_err(|_| NodeExecutionError::ExecutionFailed {
            message: "iteration index exceeds the signed 64-bit range".into(),
        })?;
        Ok(Self {
            outputs: BTreeMap::from([
                (crate::iteration::ITERATION_SEED_ITEM.into(), Some(item)),
                (
                    crate::iteration::ITERATION_SEED_INDEX.into(),
                    Some(Value::from(index)),
                ),
            ]),
            observation: None,
            body_observation,
        })
    }

    pub fn iteration_observation(
        &self,
        id: &str,
        body_nodes: &[NodeIdentity],
    ) -> Option<IterationObservation> {
        self.observation
            .as_ref()?
            .iteration_observation(id, body_nodes.to_vec())
    }

    /// Runs a synchronous execution scope without changing its result or installing providers.
    pub fn run<T, E: std::fmt::Display>(
        observation: Option<RunObservation>,
        execute: impl FnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut state = Self {
            observation,
            body_observation: None,
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
            for (name, value) in &result.outputs {
                let Some(port) = node.ports.outputs.iter().find(|port| port.name == *name) else {
                    return Err(state_error(
                        id,
                        format!("produced undeclared output `{name}`"),
                    ));
                };
                port.value_type
                    .validate_value(value)
                    .map_err(|error| state_error(id, format!("output `{name}`: {error}")))?;
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

enum StepObservation {
    Root(NodeObservation),
    Body(BodyNodeObservation),
}

impl StepObservation {
    fn started(&mut self, ctx: &mut ExecutionContext) {
        match self {
            Self::Root(step) => ctx.observation.as_mut().unwrap().node_started(step),
            Self::Body(step) => step.started(),
        }
    }

    fn failed(self, ctx: &mut ExecutionContext, phase: FailurePhase, message: String) {
        match self {
            Self::Root(step) => ctx
                .observation
                .as_mut()
                .unwrap()
                .node_failed(step, phase, message),
            Self::Body(step) => step.failed(phase, message),
        }
    }

    fn succeeded(
        self,
        ctx: &mut ExecutionContext,
        produced_ports: Vec<String>,
        skipped_ports: Vec<String>,
    ) {
        match self {
            Self::Root(step) => ctx.observation.as_mut().unwrap().node_succeeded(
                step,
                produced_ports,
                skipped_ports,
            ),
            Self::Body(step) => step.succeeded(produced_ports, skipped_ports),
        }
    }

    fn skipped(
        self,
        ctx: &mut ExecutionContext,
        causes: Vec<SkipCause>,
        skipped_ports: Vec<String>,
    ) {
        match self {
            Self::Root(step) => {
                ctx.observation
                    .as_mut()
                    .unwrap()
                    .node_skipped(step, causes, skipped_ports)
            }
            Self::Body(step) => step.skipped(causes, skipped_ports),
        }
    }
}

/// Executes one step of a validated plan, in its topological order.
pub fn execute_node_in_context(
    node: &FlowNode,
    dependencies: &[ExecutionDependency<'_>],
    ctx: &mut ExecutionContext,
) -> Result<(), WorkflowRunError> {
    let id = node.definition_id.as_str();
    let mut step = if let Some(run) = ctx.observation.as_mut() {
        run.begin_node(id).map(StepObservation::Root)
    } else {
        ctx.body_observation
            .as_ref()
            .and_then(|body| body.begin_node(id))
            .map(StepObservation::Body)
    };
    let _root_context = match &step {
        Some(StepObservation::Root(step)) => Some(step.enter()),
        _ => None,
    };
    let _body_context = match &step {
        Some(StepObservation::Body(step)) => Some(step.enter()),
        _ => None,
    };
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
                if let Some(step) = step.take() {
                    step.failed(ctx, FailurePhase::Dependency, error.to_string());
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
        for (name, value) in &inputs {
            let validation = node
                .ports
                .inputs
                .iter()
                .find(|port| port.name == *name)
                .ok_or_else(|| state_error(id, format!("received undeclared input `{name}`")))
                .and_then(|port| {
                    port.value_type
                        .validate_value(value)
                        .map_err(|error| state_error(id, format!("input `{name}`: {error}")))
                });
            if let Err(error) = validation {
                if let Some(step) = step.take() {
                    step.failed(ctx, FailurePhase::Dependency, error.to_string());
                }
                return Err(error);
            }
        }
        if let Some(step) = step.as_mut() {
            step.started(ctx);
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
                if let Some(step) = step.take() {
                    step.failed(ctx, FailurePhase::Execution, error.to_string());
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
    if let Some(step) = step.take() {
        match &result {
            Err(error) => step.failed(ctx, FailurePhase::Publication, error.to_string()),
            Ok(()) if skipped => step.skipped(ctx, causes.into_iter().collect(), skipped_ports),
            Ok(()) => step.succeeded(ctx, produced_ports, skipped_ports),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Flow, Node, NodePorts, PortSpec, ValueType, WorkflowOutputDefinition};
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct EmitNode(Outputs);

    impl Node for EmitNode {
        fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
            Ok(self.0.clone())
        }
    }

    struct CountNode(Arc<AtomicUsize>);

    impl Node for CountNode {
        fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Outputs::from([("value".into(), json!(true))]))
        }
    }

    fn port(name: &str, value_type: ValueType, required: bool) -> PortSpec {
        PortSpec::owned(name, value_type, required)
    }

    #[test]
    fn rejects_a_bad_output_without_publishing_any_result() {
        let node = FlowNode::new(
            "producer",
            Box::new(EmitNode(Outputs::from([
                ("good".into(), json!(1)),
                ("bad".into(), json!("wrong")),
            ]))),
            NodePorts {
                inputs: vec![],
                outputs: vec![
                    port("good", ValueType::Int64, true),
                    port("bad", ValueType::Int64, true),
                ],
            },
        );
        let mut context = ExecutionContext::default();
        let error = execute_node_in_context(&node, &[], &mut context)
            .unwrap_err()
            .to_string();
        assert!(error.contains("producer") && error.contains("output `bad`"));
        assert!(error.contains("expected int64, found string"));
        assert!(context.output("producer.good").is_err());
        assert!(context.output("producer.bad").is_err());
    }

    #[test]
    fn rejects_a_nested_dynamic_input_before_invoking_the_target() {
        let calls = Arc::new(AtomicUsize::new(0));
        let node = FlowNode::new(
            "consumer",
            Box::new(CountNode(Arc::clone(&calls))),
            NodePorts {
                inputs: vec![port(
                    "payload",
                    ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Int64)))),
                    true,
                )],
                outputs: vec![port("value", ValueType::Boolean, true)],
            },
        );
        let mut context = ExecutionContext::default();
        context.outputs.insert(
            "source.value".into(),
            Some(json!([{"count": 1}, {"count": "two"}])),
        );
        let dependency = ExecutionDependency {
            input: Some("payload"),
            source_node: "source",
            source_output: "value",
        };
        let error = execute_node_in_context(&node, &[dependency], &mut context)
            .unwrap_err()
            .to_string();
        assert!(error.contains("consumer") && error.contains("input `payload`"));
        assert!(error.contains("/1/count") && error.contains("expected int64"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(context.output("consumer.value").is_err());
    }

    #[test]
    fn rejects_an_undeclared_direct_flow_input() {
        let calls = Arc::new(AtomicUsize::new(0));
        let node = FlowNode::new(
            "consumer",
            Box::new(CountNode(Arc::clone(&calls))),
            NodePorts {
                inputs: vec![],
                outputs: vec![port("value", ValueType::Boolean, true)],
            },
        );
        let mut context = ExecutionContext::default();
        context
            .outputs
            .insert("source.value".into(), Some(json!(1)));
        let dependency = ExecutionDependency {
            input: Some("unexpected"),
            source_node: "source",
            source_output: "value",
        };
        let error = execute_node_in_context(&node, &[dependency], &mut context)
            .unwrap_err()
            .to_string();
        assert!(error.contains("consumer") && error.contains("undeclared input `unexpected`"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(context.output("consumer.value").is_err());
    }

    #[test]
    fn skips_without_type_checks_but_keeps_missing_output_precedence() {
        let calls = Arc::new(AtomicUsize::new(0));
        let node = FlowNode::new(
            "consumer",
            Box::new(CountNode(Arc::clone(&calls))),
            NodePorts {
                inputs: vec![port("payload", ValueType::Int64, true)],
                outputs: vec![port("value", ValueType::Boolean, true)],
            },
        );
        let skipped = ExecutionDependency {
            input: Some("payload"),
            source_node: "branch",
            source_output: "off",
        };
        let missing = ExecutionDependency {
            input: None,
            source_node: "source",
            source_output: "missing",
        };
        let mut context = ExecutionContext::default();
        context.outputs.insert("branch.off".into(), None);
        let error = execute_node_in_context(&node, &[skipped, missing], &mut context)
            .unwrap_err()
            .to_string();
        assert!(error.contains("source.missing"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(context.output("consumer.value").is_err());

        execute_node_in_context(&node, &[skipped], &mut context).unwrap();
        assert!(matches!(
            context.output("consumer.value").unwrap(),
            ContextValue::Skipped
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    struct AlternatingNode(AtomicUsize);

    impl Node for AlternatingNode {
        fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
            let value = if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                json!("wrong")
            } else {
                json!(42)
            };
            Ok(Outputs::from([("value".into(), value)]))
        }
    }

    #[test]
    fn a_failed_run_does_not_poison_the_next_run() {
        let flow = Flow::new(
            vec![FlowNode::new(
                "source",
                Box::new(AlternatingNode(AtomicUsize::new(0))),
                NodePorts {
                    inputs: vec![],
                    outputs: vec![port("value", ValueType::Int64, true)],
                },
            )],
            vec![],
            vec!["source".into()],
            vec![WorkflowOutputDefinition {
                name: "result".into(),
                node: "source".into(),
                port: "value".into(),
                optional: false,
            }],
        )
        .unwrap();
        assert!(
            flow.execute()
                .unwrap_err()
                .to_string()
                .contains("expected int64")
        );
        assert_eq!(flow.execute().unwrap()["result"], json!(42));
    }

    #[test]
    fn checks_any_source_before_a_refined_consumer_runs() {
        for (value, succeeds) in [(json!(21), true), (json!("21"), false)] {
            let calls = Arc::new(AtomicUsize::new(0));
            let flow = Flow::new(
                vec![
                    FlowNode::new(
                        "source",
                        Box::new(EmitNode(Outputs::from([("value".into(), value)]))),
                        NodePorts {
                            inputs: vec![],
                            outputs: vec![port("value", ValueType::Any, true)],
                        },
                    ),
                    FlowNode::new(
                        "consumer",
                        Box::new(CountNode(Arc::clone(&calls))),
                        NodePorts {
                            inputs: vec![port("payload", ValueType::Int64, true)],
                            outputs: vec![port("value", ValueType::Boolean, true)],
                        },
                    ),
                ],
                vec![crate::EdgeDefinition {
                    from_node: "source".into(),
                    from_output: "value".into(),
                    to_node: "consumer".into(),
                    to_input: "payload".into(),
                }],
                vec!["source".into(), "consumer".into()],
                vec![WorkflowOutputDefinition {
                    name: "result".into(),
                    node: "consumer".into(),
                    port: "value".into(),
                    optional: false,
                }],
            )
            .unwrap();
            if succeeds {
                assert_eq!(flow.execute().unwrap()["result"], json!(true));
                assert_eq!(calls.load(Ordering::SeqCst), 1);
            } else {
                let error = flow.execute().unwrap_err().to_string();
                assert!(error.contains("consumer") && error.contains("input `payload`"));
                assert_eq!(calls.load(Ordering::SeqCst), 0);
            }
        }
    }
}
