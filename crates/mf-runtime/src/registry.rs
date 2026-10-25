use crate::NodeRegistration;
use snafu::Snafu;
use std::collections::BTreeMap;

/// Keeps application-owned Flow definitions discoverable without compiler dependencies on providers.
pub struct BuiltinFlowRegistration {
    package: &'static str,
    name: &'static str,
    pub definition: fn() -> Result<crate::WorkflowDefinition, crate::DefinitionParseError>,
}

impl BuiltinFlowRegistration {
    #[doc(hidden)]
    pub const fn new(
        package: &'static str,
        name: &'static str,
        definition: fn() -> Result<crate::WorkflowDefinition, crate::DefinitionParseError>,
    ) -> Self {
        Self {
            package,
            name,
            definition,
        }
    }
}

/// Registers a local Flow name under the calling Cargo package's namespace.
#[macro_export]
macro_rules! register_builtin_flow {
    ($name:expr, $definition:path $(,)?) => {
        $crate::__submit_builtin_flow! {
            $crate::BuiltinFlowRegistration::new(env!("CARGO_PKG_NAME"), $name, $definition)
        }
    };
}

inventory::collect!(BuiltinFlowRegistration);

#[derive(Debug, Snafu)]
pub enum BuiltinFlowRegistryError {
    #[snafu(display("built-in Flow name must not be blank"))]
    BlankName,
    #[snafu(display("built-in Flow local name `{name}` must not contain a namespace separator"))]
    QualifiedLocalName { name: String },
    #[snafu(display("built-in Flow `{name}` is registered more than once"))]
    DuplicateName { name: String },
}

#[derive(Default)]
pub struct BuiltinFlowRegistry {
    registrations: BTreeMap<String, &'static BuiltinFlowRegistration>,
}

impl BuiltinFlowRegistry {
    pub fn from_inventory() -> Result<Self, BuiltinFlowRegistryError> {
        Self::from_registrations(inventory::iter::<BuiltinFlowRegistration>)
    }

    pub fn get(&self, name: &str) -> Option<&'static BuiltinFlowRegistration> {
        self.registrations.get(name).copied()
    }

    fn from_registrations(
        registrations: impl IntoIterator<Item = &'static BuiltinFlowRegistration>,
    ) -> Result<Self, BuiltinFlowRegistryError> {
        let mut registry = Self::default();
        for registration in registrations {
            snafu::ensure!(!registration.name.trim().is_empty(), BlankNameSnafu);
            snafu::ensure!(
                !registration.name.contains('.'),
                QualifiedLocalNameSnafu {
                    name: registration.name
                }
            );
            let name = format!("{}.{}", registration.package, registration.name);
            snafu::ensure!(
                registry
                    .registrations
                    .insert(name.clone(), registration)
                    .is_none(),
                DuplicateNameSnafu { name }
            );
        }
        Ok(registry)
    }
}

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
                crate::LOOP_ASSIGN_KIND | crate::EXIT_LOOP_KIND | crate::LOOP_SOURCE_ID
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

    fn builtin_definition() -> Result<crate::WorkflowDefinition, crate::DefinitionParseError> {
        crate::WorkflowDefinition::from_json(
            r#"{"version":"2026-10-25","dependencies":{},"nodes":[]}"#,
        )
    }

    #[test]
    fn builtin_catalog_rejects_duplicate_and_blank_names() {
        static FIRST: BuiltinFlowRegistration = BuiltinFlowRegistration {
            package: "first-package",
            name: "flow",
            definition: builtin_definition,
        };
        static SECOND: BuiltinFlowRegistration = BuiltinFlowRegistration {
            package: "first-package",
            name: "flow",
            definition: builtin_definition,
        };
        static BLANK: BuiltinFlowRegistration = BuiltinFlowRegistration {
            package: "first-package",
            name: " ",
            definition: builtin_definition,
        };
        assert!(matches!(
            BuiltinFlowRegistry::from_registrations([&FIRST, &SECOND]),
            Err(BuiltinFlowRegistryError::DuplicateName { .. })
        ));
        assert!(matches!(
            BuiltinFlowRegistry::from_registrations([&BLANK]),
            Err(BuiltinFlowRegistryError::BlankName)
        ));
        static OTHER_PACKAGE: BuiltinFlowRegistration = BuiltinFlowRegistration {
            package: "second-package",
            name: "flow",
            definition: builtin_definition,
        };
        static QUALIFIED: BuiltinFlowRegistration = BuiltinFlowRegistration {
            package: "first-package",
            name: "other-package.flow",
            definition: builtin_definition,
        };
        assert!(matches!(
            BuiltinFlowRegistry::from_registrations([&QUALIFIED]),
            Err(BuiltinFlowRegistryError::QualifiedLocalName { .. })
        ));
        let registry = BuiltinFlowRegistry::from_registrations([&FIRST, &OTHER_PACKAGE]).unwrap();
        assert!(registry.get("missing").is_none());
        assert!(registry.get("flow").is_none());
        assert!(registry.get("second-package.flow").is_some());
        assert!(
            (registry.get("first-package.flow").unwrap().definition)()
                .unwrap()
                .nodes
                .is_empty()
        );
    }

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
        let metadata = crate::NodeMetadata::new(crate::NodePorts {
            inputs: vec![],
            outputs: vec![],
        });
        Ok(crate::PreparedNode::from_parts(
            crate::NodeExecution::Task(Box::new(node)),
            metadata,
        ))
    }

    fn second_factory(_config: Value) -> Result<crate::PreparedNode, NodeBuildError> {
        let node = SecondTestNode;
        let metadata = crate::NodeMetadata::new(crate::NodePorts {
            inputs: vec![],
            outputs: vec![crate::PortSpec::new("value", ValueType::Any, false)],
        });
        Ok(crate::PreparedNode::from_parts(
            crate::NodeExecution::Task(Box::new(node)),
            metadata,
        ))
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
