use mf_runtime::{BuiltinFlowRegistry, DefinitionParseError, WorkflowDefinition};
use std::sync::atomic::{AtomicUsize, Ordering};

static FACTORY_CALLS: AtomicUsize = AtomicUsize::new(0);

fn definition() -> Result<WorkflowDefinition, DefinitionParseError> {
    FACTORY_CALLS.fetch_add(1, Ordering::SeqCst);
    WorkflowDefinition::from_json(r#"{"version":"2026-10-23","dependencies":{},"nodes":[]}"#)
}

fn invalid_definition() -> Result<WorkflowDefinition, DefinitionParseError> {
    WorkflowDefinition::from_json("{}")
}

mf_runtime::register_builtin_flow!("example", definition,);
mf_runtime::register_builtin_flow!("invalid", invalid_definition);

#[test]
fn registration_uses_the_calling_package_and_defers_definition_factories() {
    assert_eq!(FACTORY_CALLS.load(Ordering::SeqCst), 0);
    let registry = BuiltinFlowRegistry::from_inventory().unwrap();
    assert_eq!(FACTORY_CALLS.load(Ordering::SeqCst), 0);
    assert!(registry.get("example").is_none());
    assert!(registry.get("mf-runtime.example").is_none());
    let registration = registry.get("mf-compiler.example").unwrap();
    let definition = (registration.definition)().unwrap();
    assert!(definition.nodes.is_empty());
    assert_eq!(FACTORY_CALLS.load(Ordering::SeqCst), 1);
    assert!((registry.get("mf-compiler.invalid").unwrap().definition)().is_err());
}
