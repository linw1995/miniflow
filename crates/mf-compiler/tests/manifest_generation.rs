#![cfg(feature = "codegen")]

use mf_compiler::{NodeRegistry, WorkflowDefinition, describe_workflow_inputs, plan_definition};
use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodePorts, NodeRegistration, NodeResult, PortSpec, PreparedNode, StdinRequirement, TaskNode,
    ValueType, WorkflowManifest,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

static PREPARATIONS: AtomicUsize = AtomicUsize::new(0);

struct NeverRun;
impl TaskNode for NeverRun {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        panic!("build-time metadata generation must not execute nodes");
    }
}
impl mf_runtime::StreamNode for NeverRun {
    fn execute(
        &mut self,
        _: Inputs,
        _: &mut ExecutionContext,
        _: &mut mf_runtime::Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        panic!("build-time metadata generation must not consume sources");
    }
}

fn prepare(config: Value) -> Result<PreparedNode, NodeBuildError> {
    PREPARATIONS.fetch_add(1, Ordering::SeqCst);
    let port = config["port"].as_str().unwrap().to_owned();
    let metadata = NodeMetadata {
        stdin: config["stdin"]
            .as_bool()
            .unwrap_or(false)
            .then(|| StdinRequirement::UnlessInput(port.clone())),
        ..NodeMetadata::new(NodePorts {
            inputs: vec![PortSpec::owned(port, ValueType::String, false)],
            outputs: vec![PortSpec::new("value", ValueType::String, true)],
        })
    };
    Ok(if config["stream"].as_bool().unwrap_or(false) {
        PreparedNode::stream(NeverRun, metadata)
    } else {
        PreparedNode::new(NeverRun, metadata)
    })
}

inventory::submit! {
    NodeRegistration { kind: "fixture.manifest", factory: NodeFactory::Plain(prepare) }
}

#[test]
fn generates_layouts_and_configured_interfaces_from_one_preparation() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (version, stream) in [
        ("2026-09-26", false),
        ("2026-10-03", false),
        ("2026-10-03", true),
    ] {
        let mut definition = json!({
            "version": version,
            "dependencies": {},
            "nodes": [{"id": "source./~", "kind": "fixture.manifest", "config": {
                "port": "path./~", "stdin": version == "2026-10-03", "stream": stream
            }}],
            "outputs": [{"name": "result", "node": "source./~", "port": "value"}]
        });
        if stream {
            definition["execution"] = json!({"mode": "stream"});
        }
        let definition = WorkflowDefinition::from_json(&definition.to_string()).unwrap();
        let plan = plan_definition(&definition).unwrap();
        let before = PREPARATIONS.load(Ordering::SeqCst);
        let artifacts = plan.generate_execution_plans(&registry).unwrap();
        assert_eq!(PREPARATIONS.load(Ordering::SeqCst), before + 1);
        syn::parse_file(&artifacts.rust_source).unwrap();
        let manifest = WorkflowManifest::from_bytes(&artifacts.manifest_bytes).unwrap();
        assert_eq!(manifest.description.is_streaming(), stream);
        assert_eq!(
            manifest.interface.schema,
            describe_workflow_inputs(&plan, &registry).unwrap()
        );
        if version == "2026-10-03" {
            assert_eq!(
                manifest.interface.schema.inputs["source./~"]["path./~"].value_type,
                ValueType::String
            );
            assert_eq!(
                manifest.interface.schema.stdin["source./~"],
                StdinRequirement::UnlessInput("path./~".into())
            );
        } else {
            assert_eq!(manifest.interface.schema, Default::default());
        }
        let repeated = plan.generate_execution_plans(&registry).unwrap();
        assert_eq!(artifacts, repeated);
    }
}
