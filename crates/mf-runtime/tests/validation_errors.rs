use mf_runtime::{
    LoopVariableDefinition, LoopVariableError, NodeDependency, NodeDependencyError, TypeDepthError,
    TypeDescriptorError, ValueType, loop_variable_types,
};
use serde_json::json;
use std::error::Error;

#[test]
fn nested_descriptors_preserve_the_depth_error() {
    let mut descriptor = json!("int");
    for _ in 0..ValueType::MAX_DEPTH {
        descriptor = json!({"list": descriptor});
    }
    let error = ValueType::parse_descriptor(&descriptor).unwrap_err();
    let source = error
        .source()
        .unwrap()
        .downcast_ref::<TypeDepthError>()
        .unwrap();
    assert_eq!(source.depth, ValueType::MAX_DEPTH + 1);
    assert_eq!(source.maximum, ValueType::MAX_DEPTH);
}

#[test]
fn descriptor_failures_distinguish_names_constructors_and_shapes() {
    assert!(matches!(ValueType::parse_descriptor(&json!("unknown")),
        Err(TypeDescriptorError::UnsupportedName { name }) if name == "unknown"));
    assert!(
        matches!(ValueType::parse_descriptor(&json!({"unknown": "int"})),
        Err(TypeDescriptorError::UnsupportedConstructor { kind }) if kind == "unknown")
    );
    assert!(matches!(
        ValueType::parse_descriptor(&json!({})),
        Err(TypeDescriptorError::InvalidDescriptor)
    ));
}

#[test]
fn loop_variable_failures_preserve_the_descriptor_source() {
    let error = loop_variable_types(&[LoopVariableDefinition {
        name: "count".into(),
        value_type: json!({"list": "unknown"}),
    }])
    .unwrap_err();
    assert!(
        matches!(&error, LoopVariableError::VariableType { variable, .. } if variable == "count")
    );
    assert!(
        matches!(error.source().unwrap().downcast_ref::<TypeDescriptorError>().unwrap(),
        TypeDescriptorError::UnsupportedName { name } if name == "unknown")
    );
}

#[test]
fn dependency_validation_returns_structured_failures() {
    let dependency: NodeDependency =
        serde_json::from_value(json!({"package": "example", "git": "https://example.com/repo"}))
            .unwrap();
    assert_eq!(
        dependency.validate(),
        Err(NodeDependencyError::InvalidRevision)
    );
    let dependency: NodeDependency =
        serde_json::from_value(json!({"package": "example", "version": "1", "path": "."})).unwrap();
    assert_eq!(
        dependency.validate(),
        Err(NodeDependencyError::InvalidSources)
    );
}
