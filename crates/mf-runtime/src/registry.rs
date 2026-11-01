use crate::NodeRegistration;
use snafu::Snafu;
use std::collections::BTreeMap;

/// Configurable provider expansion, supplied resolved, frozen injections and their metadata.
pub type BuiltinFlowExpansion = fn(
    serde_json::Value,
    BTreeMap<String, crate::ResolvedFlowArgument>,
) -> Result<crate::WorkflowDefinition, crate::NodeBuildError>;

/// Keeps application-owned Flow definitions discoverable without compiler dependencies on providers.
pub struct BuiltinFlowRegistration {
    package: &'static str,
    name: &'static str,
    pub definition: fn() -> Result<crate::WorkflowDefinition, crate::DefinitionParseError>,
    pub configure: Option<BuiltinFlowExpansion>,
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
            configure: None,
        }
    }
    pub const fn configurable(
        package: &'static str,
        name: &'static str,
        definition: fn() -> Result<crate::WorkflowDefinition, crate::DefinitionParseError>,
        configure: BuiltinFlowExpansion,
    ) -> Self {
        Self {
            package,
            name,
            definition,
            configure: Some(configure),
        }
    }
}

/// Registers a local Flow name under the calling Cargo package's namespace.
#[macro_export]
macro_rules! register_builtin_flow {
    ($name:expr, $definition:path, $configure:path $(,)?) => {
        $crate::__submit_builtin_flow! {
            $crate::BuiltinFlowRegistration::configurable(env!("CARGO_PKG_NAME"), $name, $definition, $configure)
        }
    };
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
    #[snafu(display("host Flow name must not be blank"))]
    BlankFlowName,
    #[snafu(display("host Flow `{name}` is registered more than once"))]
    DuplicateFlow { name: String },
    #[snafu(display("node kind `{kind}` is registered more than once"))]
    DuplicateKind { kind: String },
    #[snafu(display("node kind `{kind}` is missing from the linked plugin bundle"))]
    MissingKind { kind: String },
    #[snafu(display("node kind `{kind}` is linked but absent from the selected plugin bundle"))]
    UnexpectedKind { kind: String },
    #[snafu(display("node kind `{kind}` is reserved for workflow control"))]
    ReservedKind { kind: String },
}

/// A frozen Flow whose execution is supplied by the application host.
pub struct RegisteredFlow {
    pub definition: crate::WorkflowDefinition,
    pub body: crate::PreparedSubgraph,
}

impl std::fmt::Debug for RegisteredFlow {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RegisteredFlow")
            .field("definition", &self.definition)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
pub struct NodeRegistry {
    registrations: BTreeMap<&'static str, &'static NodeRegistration>,
    flows: BTreeMap<String, RegisteredFlow>,
}

impl NodeRegistry {
    /// Binds a named, frozen Flow to a host executor without requiring its node providers.
    /// The executor may be shared by multiple call sites and must isolate per-call state.
    pub fn register_flow(
        &mut self,
        name: impl Into<String>,
        definition: crate::WorkflowDefinition,
        body: crate::PreparedSubgraph,
    ) -> Result<(), NodeRegistryError> {
        let name = name.into();
        snafu::ensure!(!name.trim().is_empty(), BlankFlowNameSnafu);
        snafu::ensure!(!self.flows.contains_key(&name), DuplicateFlowSnafu { name });
        self.flows.insert(name, RegisteredFlow { definition, body });
        Ok(())
    }

    pub fn flow(&self, name: &str) -> Option<&RegisteredFlow> {
        self.flows.get(name)
    }

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
            snafu::ensure!(
                !matches!(
                    registration.kind,
                    crate::LOOP_ASSIGN_KIND | crate::EXIT_LOOP_KIND | crate::LOOP_SOURCE_ID
                ),
                ReservedKindSnafu {
                    kind: registration.kind
                }
            );
            snafu::ensure!(
                registry
                    .registrations
                    .insert(registration.kind, registration)
                    .is_none(),
                DuplicateKindSnafu {
                    kind: registration.kind
                }
            );
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
            r#"{"version":"2026-10-23","dependencies":{},"nodes":[]}"#,
        )
    }

    #[test]
    fn builtin_catalog_rejects_duplicate_and_blank_names() {
        static REGISTRATIONS: std::sync::LazyLock<[BuiltinFlowRegistration; 5]> =
            std::sync::LazyLock::new(|| {
                [
                    BuiltinFlowRegistration::new("first-package", "flow", builtin_definition),
                    BuiltinFlowRegistration::new("first-package", "flow", builtin_definition),
                    BuiltinFlowRegistration::new("first-package", " ", builtin_definition),
                    BuiltinFlowRegistration::new("second-package", "flow", builtin_definition),
                    BuiltinFlowRegistration::new(
                        "first-package",
                        "other-package.flow",
                        builtin_definition,
                    ),
                ]
            });
        let [first, duplicate, blank, other_package, qualified] = &*REGISTRATIONS;
        assert!(matches!(
            BuiltinFlowRegistry::from_registrations([first, duplicate]),
            Err(BuiltinFlowRegistryError::DuplicateName { name }) if name == "first-package.flow"
        ));
        assert!(matches!(
            BuiltinFlowRegistry::from_registrations([blank]),
            Err(BuiltinFlowRegistryError::BlankName)
        ));
        assert!(matches!(
            BuiltinFlowRegistry::from_registrations([qualified]),
            Err(BuiltinFlowRegistryError::QualifiedLocalName { .. })
        ));
        let registry = BuiltinFlowRegistry::from_registrations([first, other_package]).unwrap();
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
