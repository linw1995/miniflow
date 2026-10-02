use mf_compiler::WorkflowDefinition;
use mf_compiler::{
    Inputs, NodeBuildError, NodeExecutionError, NodeRegistration, NodeRegistry, Outputs, PortSpec,
    TaskNode, ValueType, WorkflowCompileError, deserialize_config, resolve_nodes,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::error::Error;

#[derive(Deserialize)]
struct ConstantConfig {
    value: i64,
}

struct ConstantNode {
    value: i64,
}

impl TaskNode for ConstantNode {
    fn execute(
        &self,
        _inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok((Outputs::from([("value".to_owned(), json!(self.value).into())])).into())
    }
}

fn constant_factory(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let config: ConstantConfig = deserialize_config(config)?;
    let node = ConstantNode {
        value: config.value,
    };
    let metadata = mf_runtime::NodeMetadata {
        ports: mf_runtime::NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::new("value", ValueType::Number, true)],
        },
        output_derivations: Vec::new(),
        context_references: Vec::new(),
    };
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

inventory::submit! {
    NodeRegistration { kind: "test.constant", factory: mf_runtime::NodeFactory::Plain(constant_factory) }
}

#[test]
fn resolves_registered_nodes_in_definition_order() {
    let definition = WorkflowDefinition::from_json(
        r#"{
            "version": "2026-09-26", "dependencies": {},
            "nodes": [
                {"id":"first","kind":"test.constant","config":{"value":7}},
                {"id":"second","kind":"test.constant","config":{"value":11}}
            ]
        }"#,
    )
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();

    let nodes = resolve_nodes(&definition, &registry).unwrap();

    assert_eq!(nodes[0].definition_id.as_str(), "first");
    assert_eq!(nodes[1].definition_id.as_str(), "second");
    assert_eq!(
        nodes[0]
            .node
            .execute(Inputs::new(), &mut mf_runtime::ExecutionContext::default())
            .unwrap()
            .outputs["value"],
        json!(7)
    );
    assert_eq!(
        nodes[1]
            .node
            .execute(Inputs::new(), &mut mf_runtime::ExecutionContext::default())
            .unwrap()
            .outputs["value"],
        json!(11)
    );
}

#[test]
fn reports_unknown_kind_with_definition_id() {
    let definition = WorkflowDefinition::from_json(
        r#"{"version": "2026-09-26", "dependencies": {},"nodes":[{"id":"missing-plugin","kind":"test.unknown"}]}"#,
    )
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();

    let error = resolve_nodes(&definition, &registry).err().unwrap();

    assert!(matches!(
        &error,
        WorkflowCompileError::UnknownNodeKind { definition_id, kind }
            if definition_id.as_str() == "missing-plugin" && kind == "test.unknown"
    ));
    assert!(error.to_string().contains("`missing-plugin`"));
}

#[test]
fn reports_invalid_config_with_definition_id_and_source() {
    let definition = WorkflowDefinition::from_json(
        r#"{"version": "2026-09-26", "dependencies": {},"nodes":[{"id":"bad-config","kind":"test.constant","config":{"value":"wrong"}}]}"#,
    )
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();

    let error = resolve_nodes(&definition, &registry).err().unwrap();

    assert!(matches!(
        &error,
        WorkflowCompileError::NodeConstruction { definition_id, .. }
            if definition_id.as_str() == "bad-config"
    ));
    assert!(error.to_string().contains("`bad-config`"));
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<NodeBuildError>()
            .is_some()
    );
}
