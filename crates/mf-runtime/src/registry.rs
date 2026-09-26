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
    use crate::{Inputs, Node, NodeBuildError, NodeExecutionError, Outputs, ValueType};
    use serde_json::Value;

    struct FirstTestNode;

    impl Node for FirstTestNode {
        fn execute(&self, _inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
            Ok(Outputs::new())
        }
    }

    struct SecondTestNode;

    impl Node for SecondTestNode {
        fn execute(&self, _inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
            Ok(Outputs::new())
        }
    }

    fn first_factory(_config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
        Ok(Box::new(FirstTestNode))
    }

    fn second_factory(_config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
        Ok(Box::new(SecondTestNode))
    }

    static FIRST: NodeRegistration = NodeRegistration {
        kind: "test.duplicate",
        inputs: &[],
        outputs: &[],
        factory: first_factory,
    };

    static SECOND: NodeRegistration = NodeRegistration {
        kind: "test.duplicate",
        inputs: &[],
        outputs: &[crate::PortSpec::new("value", ValueType::Any, false)],
        factory: second_factory,
    };

    #[test]
    fn rejects_duplicate_node_kinds() {
        let error = NodeRegistry::from_registrations([&FIRST, &SECOND]).unwrap_err();

        assert!(
            matches!(error, NodeRegistryError::DuplicateKind { ref kind } if kind == "test.duplicate")
        );
    }
}
