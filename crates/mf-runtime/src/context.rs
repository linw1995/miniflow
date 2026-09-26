use crate::{
    ContextReference, FlowNode, Inputs, NodeExecutionError, NodePorts, Outputs, WorkflowRunError,
    output_id,
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

#[derive(Debug)]
enum Outcome {
    Completed(NodeResult),
    Skipped,
}

#[derive(Debug)]
struct Entry {
    ports: Option<NodePorts>,
    outcome: Option<Outcome>,
}

#[derive(Debug, Default)]
pub struct ExecutionState {
    nodes: BTreeMap<String, Entry>,
    output_index: BTreeMap<String, (String, String)>,
}

pub struct ExecutionContext<'a> {
    state: &'a ExecutionState,
    allowed: &'a [ContextReference],
}

impl ExecutionContext<'_> {
    pub fn output(&self, id: &str) -> Result<ContextValue<'_>, NodeExecutionError> {
        let error = |message| NodeExecutionError::ExecutionFailed { message };
        if !self.allowed.iter().any(|reference| reference.output == id) {
            return Err(error(format!("undeclared context output `{id}`")));
        }
        let Some((node, port)) = self.state.output_index.get(id) else {
            return Err(error(format!("unknown context output `{id}`")));
        };
        self.state.lookup(node, port).map_err(error)
    }
}

impl ExecutionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, node: &FlowNode) -> Result<(), WorkflowRunError> {
        let id = node.definition_id.as_str();
        if self.nodes.contains_key(id) {
            return Err(state_error(id, "node registered more than once"));
        }
        let mut keys = Vec::new();
        if let Some(ports) = &node.ports {
            for specs in [&ports.inputs, &ports.outputs] {
                let mut names = BTreeSet::new();
                for port in specs {
                    if port.name.is_empty() || !names.insert(&port.name) {
                        return Err(state_error(
                            id,
                            format!("empty or duplicate port `{}`", port.name),
                        ));
                    }
                }
            }
            for port in &ports.outputs {
                let key = output_id(id, &port.name);
                if let Some((other, name)) = self.output_index.get(&key) {
                    return Err(state_error(
                        id,
                        format!("output ID `{key}` collides with `{other}`.`{name}`"),
                    ));
                }
                keys.push((key, (id.to_owned(), port.name.clone())));
            }
        }
        self.output_index.extend(keys);
        self.nodes.insert(
            id.to_owned(),
            Entry {
                ports: node.ports.clone(),
                outcome: None,
            },
        );
        Ok(())
    }

    fn lookup(&self, node: &str, port: &str) -> Result<ContextValue<'_>, String> {
        let key = output_id(node, port);
        let entry = self
            .nodes
            .get(node)
            .ok_or_else(|| format!("unknown source node `{node}` for `{key}`"))?;
        if let Some(ports) = &entry.ports
            && !ports.outputs.iter().any(|spec| spec.name == port)
        {
            return Err(format!("unknown output `{key}`"));
        }
        match &entry.outcome {
            None => Err(format!("output `{key}` is pending")),
            Some(Outcome::Skipped) => Ok(ContextValue::Skipped),
            Some(Outcome::Completed(result)) => {
                if let Some(value) = result.outputs.get(port) {
                    Ok(ContextValue::Value(value))
                } else if result.skipped.contains(port) {
                    Ok(ContextValue::Skipped)
                } else {
                    Err(format!("missing output `{key}`"))
                }
            }
        }
    }

    fn publish(&mut self, id: &str, result: Option<NodeResult>) -> Result<(), WorkflowRunError> {
        let entry = self
            .nodes
            .get(id)
            .ok_or_else(|| state_error(id, "node has no execution metadata"))?;
        if entry.outcome.is_some() {
            return Err(state_error(id, "node has already resolved"));
        }
        let mut keys = Vec::new();
        if let Some(result) = &result {
            for port in &result.skipped {
                if result.outputs.contains_key(port) {
                    return Err(state_error(
                        id,
                        format!("output `{port}` is both produced and skipped"),
                    ));
                }
                let Some(ports) = &entry.ports else {
                    return Err(state_error(
                        id,
                        "explicit skips require resolved port metadata",
                    ));
                };
                if !ports
                    .outputs
                    .iter()
                    .any(|spec| spec.name == *port && !spec.required)
                {
                    return Err(state_error(
                        id,
                        format!("cannot explicitly skip unknown or required output `{port}`"),
                    ));
                }
            }
            for port in result.outputs.keys() {
                if let Some(ports) = &entry.ports
                    && !ports.outputs.iter().any(|spec| spec.name == *port)
                {
                    return Err(state_error(
                        id,
                        format!("produced undeclared output `{port}`"),
                    ));
                }
                let key = output_id(id, port);
                if let Some((other, name)) = self.output_index.get(&key)
                    && (other != id || name != port)
                {
                    return Err(state_error(
                        id,
                        format!("output ID `{key}` collides with `{other}`.`{name}`"),
                    ));
                }
                keys.push((key, (id.to_owned(), port.clone())));
            }
        }
        self.output_index.extend(keys);
        self.nodes.get_mut(id).unwrap().outcome = Some(match result {
            Some(result) => Outcome::Completed(result),
            None => Outcome::Skipped,
        });
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

pub fn execute_node_in_context(
    node: &FlowNode,
    dependencies: &[ExecutionDependency<'_>],
    state: &mut ExecutionState,
) -> Result<(), WorkflowRunError> {
    let id = node.definition_id.as_str();
    match state.nodes.get(id) {
        Some(entry) if entry.outcome.is_none() => {}
        _ => {
            return Err(state_error(
                id,
                "node must be registered and unresolved before execution",
            ));
        }
    }
    let mut dependencies = dependencies.to_vec();
    dependencies.sort();
    let mut inputs = Inputs::new();
    let mut skipped = false;
    for dependency in dependencies {
        match state
            .lookup(dependency.source_node, dependency.source_output)
            .map_err(|message| {
                state_error(
                    id,
                    format!(
                        "dependency {}: {message}",
                        dependency.input.unwrap_or("<control>")
                    ),
                )
            })? {
            ContextValue::Value(value) => {
                if let Some(input) = dependency.input {
                    inputs.insert(input.to_owned(), value.clone());
                }
            }
            ContextValue::Skipped => skipped = true,
        }
    }
    let result = if skipped {
        None
    } else {
        let ctx = ExecutionContext {
            state,
            allowed: &node.references,
        };
        Some(
            node.node
                .execute_with_context(inputs, &ctx)
                .map_err(|source| WorkflowRunError::NodeExecution {
                    definition_id: node.definition_id.clone(),
                    source,
                })?,
        )
    };
    state.publish(id, result)
}

pub fn required_context_output(
    state: &ExecutionState,
    node: &str,
    port: &str,
) -> Result<Value, WorkflowRunError> {
    match state
        .lookup(node, port)
        .map_err(|message| state_error(node, message))?
    {
        ContextValue::Value(value) => Ok(value.clone()),
        ContextValue::Skipped => Err(state_error(
            node,
            format!("required output `{}` was skipped", output_id(node, port)),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, OwnedPortSpec, ValueType};
    use serde_json::json;

    struct Empty;
    impl Node for Empty {
        fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
            Ok(Outputs::new())
        }
    }
    fn node(id: &str) -> FlowNode {
        FlowNode::new(id, Box::new(Empty)).with_ports(NodePorts {
            inputs: vec![],
            outputs: vec![OwnedPortSpec::new("value", ValueType::Any, false)],
        })
    }
    #[test]
    fn separates_pending_skipped_missing_null_and_undeclared_reads() {
        let mut state = ExecutionState::new();
        for id in ["pending", "skipped", "missing", "null"] {
            state.register(&node(id)).unwrap();
        }
        state.publish("skipped", None).unwrap();
        state
            .publish("missing", Some(NodeResult::default()))
            .unwrap();
        state
            .publish(
                "null",
                Some(Outputs::from([("value".into(), json!(null))]).into()),
            )
            .unwrap();
        let references = [
            "pending.value",
            "skipped.value",
            "missing.value",
            "null.value",
            "unknown.value",
        ]
        .map(|output| ContextReference::new(output, "test"));
        let ctx = ExecutionContext {
            state: &state,
            allowed: &references,
        };
        assert!(
            ctx.output("pending.value")
                .unwrap_err()
                .to_string()
                .contains("pending")
        );
        assert!(
            ctx.output("missing.value")
                .unwrap_err()
                .to_string()
                .contains("missing")
        );
        assert!(
            ctx.output("unknown.value")
                .unwrap_err()
                .to_string()
                .contains("unknown")
        );
        assert!(
            ctx.output("other.value")
                .unwrap_err()
                .to_string()
                .contains("undeclared")
        );
        assert_eq!(ctx.output("skipped.value").unwrap(), ContextValue::Skipped);
        assert_eq!(
            ctx.output("null.value").unwrap(),
            ContextValue::Value(&Value::Null)
        );
    }
    #[test]
    fn publication_is_atomic_and_requires_skip_metadata() {
        let mut state = ExecutionState::new();
        state.register(&node("a")).unwrap();
        let bad = NodeResult {
            outputs: Outputs::from([("value".into(), json!(3))]),
            skipped: BTreeSet::from(["value".into()]),
        };
        assert!(state.publish("a", Some(bad)).is_err());
        assert!(state.lookup("a", "value").unwrap_err().contains("pending"));
        state
            .publish(
                "a",
                Some(Outputs::from([("value".into(), json!(4))]).into()),
            )
            .unwrap();
        assert!(state.publish("a", None).is_err());
        assert_eq!(
            state.lookup("a", "value").unwrap(),
            ContextValue::Value(&json!(4))
        );
        state
            .register(&FlowNode::new("legacy", Box::new(Empty)))
            .unwrap();
        let skip = NodeResult {
            outputs: Outputs::new(),
            skipped: BTreeSet::from(["value".into()]),
        };
        assert!(
            state
                .publish("legacy", Some(skip))
                .unwrap_err()
                .to_string()
                .contains("metadata")
        );
    }
    #[test]
    fn lower_level_registration_rejects_output_collisions() {
        let mut state = ExecutionState::new();
        state.register(&node("a.b")).unwrap();
        let other = FlowNode::new("a", Box::new(Empty)).with_ports(NodePorts {
            inputs: vec![],
            outputs: vec![OwnedPortSpec::new("b.value", ValueType::Any, false)],
        });
        assert!(
            state
                .register(&other)
                .unwrap_err()
                .to_string()
                .contains("a.b.value")
        );
        assert!(state.register(&node("a.b")).is_err());
    }
}
