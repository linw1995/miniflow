use crate::NodeRegistration;
use snafu::Snafu;
use std::collections::BTreeMap;

#[derive(Debug, Snafu)]
pub enum NodeRegistryError {
    #[snafu(display("node kind `{kind}` is registered more than once"))]
    DuplicateKind { kind: String },
    #[snafu(display("node kind `{kind}` is missing from the linked plugin bundle"))]
    MissingKind { kind: String },
    #[snafu(display("node kind `{kind}` is linked but absent from the selected plugin bundle"))]
    UnexpectedKind { kind: String },
    #[snafu(display("node kind `{kind}` is reserved for workflow control"))]
    ReservedKind { kind: String },
}

#[derive(Debug, Default)]
pub struct NodeRegistry {
    registrations: BTreeMap<&'static str, &'static NodeRegistration>,
}

impl NodeRegistry {
    pub fn from_inventory() -> Result<Self, NodeRegistryError> {
        Self::from_registrations(inventory::iter::<NodeRegistration>)
    }

    pub fn get(&self, kind: &str) -> Option<&'static NodeRegistration> {
        self.registrations.get(kind).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = &'static NodeRegistration> + '_ {
        self.registrations.values().copied()
    }

    fn from_registrations(
        registrations: impl IntoIterator<Item = &'static NodeRegistration>,
    ) -> Result<Self, NodeRegistryError> {
        let mut registry = Self::default();

        for registration in registrations {
            if matches!(
                registration.kind,
                crate::LOOP_ASSIGN_KIND
                    | crate::EXIT_LOOP_KIND
                    | crate::LOOP_SOURCE_ID
                    | crate::STREAM_INPUT_ID
            ) {
                return Err(NodeRegistryError::ReservedKind {
                    kind: registration.kind.to_owned(),
                });
            }
            if registry
                .registrations
                .insert(registration.kind, registration)
                .is_some()
            {
                return Err(NodeRegistryError::DuplicateKind {
                    kind: registration.kind.to_owned(),
                });
            }
        }

        Ok(registry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Inputs, NodeBuildError, NodeExecutionError, Outputs, TaskNode, ValueType};
    use serde_json::Value;

    struct FirstTestNode;

    impl TaskNode for FirstTestNode {
        fn execute(
            &self,
            _inputs: Inputs,
            _ctx: &mut crate::ExecutionContext,
        ) -> Result<crate::NodeResult, NodeExecutionError> {
            Ok(Outputs::new().into())
        }
    }

    struct SecondTestNode;

    impl TaskNode for SecondTestNode {
        fn execute(
            &self,
            _inputs: Inputs,
            _ctx: &mut crate::ExecutionContext,
        ) -> Result<crate::NodeResult, NodeExecutionError> {
            Ok(Outputs::new().into())
        }
    }

    fn first_factory(_config: Value) -> Result<crate::PreparedNode, NodeBuildError> {
        let node = FirstTestNode;
        let metadata = crate::NodeMetadata {
            ports: crate::NodePorts {
                inputs: vec![],
                outputs: vec![],
            },
            output_derivations: Vec::new(),
            resources: Vec::new(),
            context_references: Vec::new(),
        };
        Ok(crate::PreparedNode::new(node, metadata))
    }

    fn second_factory(_config: Value) -> Result<crate::PreparedNode, NodeBuildError> {
        let node = SecondTestNode;
        let metadata = crate::NodeMetadata {
            ports: crate::NodePorts {
                inputs: vec![],
                outputs: vec![crate::PortSpec::new("value", ValueType::Any, false)],
            },
            output_derivations: Vec::new(),
            resources: Vec::new(),
            context_references: Vec::new(),
        };
        Ok(crate::PreparedNode::new(node, metadata))
    }

    static FIRST: NodeRegistration = NodeRegistration {
        kind: "test.duplicate",
        factory: crate::NodeFactory::Plain(first_factory),
    };

    static SECOND: NodeRegistration = NodeRegistration {
        kind: "test.duplicate",
        factory: crate::NodeFactory::Plain(second_factory),
    };

    static RESERVED: NodeRegistration = NodeRegistration {
        kind: crate::LOOP_ASSIGN_KIND,
        factory: crate::NodeFactory::Plain(first_factory),
    };

    #[test]
    fn plain_factory_rejects_a_subgraph_during_preparation() {
        let body = crate::PreparedSubgraph::new(Vec::new(), Vec::new(), |_| Ok(Outputs::new()));
        let error = FIRST
            .instantiate_subgraph("node", Value::Null, Value::Null, body)
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("does not accept a prepared body")
        );
    }

    #[test]
    fn rejects_duplicate_node_kinds() {
        let error = NodeRegistry::from_registrations([&FIRST, &SECOND]).unwrap_err();

        assert!(
            matches!(error, NodeRegistryError::DuplicateKind { ref kind } if kind == "test.duplicate")
        );
    }

    #[test]
    fn rejects_reserved_workflow_kinds() {
        assert!(matches!(
            NodeRegistry::from_registrations([&RESERVED]),
            Err(NodeRegistryError::ReservedKind { .. })
        ));
    }
}
