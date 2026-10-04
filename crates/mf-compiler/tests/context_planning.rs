use mf_compiler::{
    ContextReference, Inputs, NodeBuildError, NodeExecutionError, NodePorts, NodeRegistration,
    NodeRegistry, Outputs, PortSpec, TaskNode, ValueType, WorkflowDefinition, compile_definition,
    instantiate_compiled, plan_definition, validate_definition,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

static CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);
struct Dynamic(Value);
impl TaskNode for Dynamic {
    fn execute(
        &self,
        _: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok(Outputs::new().into())
    }
}
impl Dynamic {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![],
            outputs: self.0["ports"]
                .as_array()
                .unwrap()
                .iter()
                .map(|name| PortSpec::owned(name.as_str().unwrap(), ValueType::Any, false))
                .collect(),
        }
    }
    fn context_references(&self) -> Vec<ContextReference> {
        self.0
            .get("reference")
            .and_then(Value::as_str)
            .map(|output| vec![ContextReference::new(output, "branch-test")])
            .unwrap_or_default()
    }
}
fn factory(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    if config["count"] == true {
        CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);
    }
    let node = Dynamic(config);
    let metadata = mf_runtime::NodeMetadata {
        ports: node.ports(),
        output_derivations: Vec::new(),
        stdin: None,
        context_references: node.context_references(),
    };
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}
inventory::submit! { NodeRegistration { kind: "test.dynamic", factory: mf_runtime::NodeFactory::Plain(factory) } }
fn definition(value: Value) -> WorkflowDefinition {
    serde_json::from_value(value).unwrap()
}
fn graph() -> Value {
    json!({"version":"2026-09-26","dependencies":{},
    "nodes":[
        {"id":"a", "kind":"test.dynamic", "config":{"ports":["value"]}},
        {"id":"b", "kind":"test.dynamic", "config":{"ports":["done"]}},
        {"id":"c", "kind":"test.dynamic", "config":{"ports":["yes","else"],"reference":"a.value"}}
    ],
    "control_edges":[
        {"from_node":"a","from_output":"value","to_node":"b"},
        {"from_node":"b","from_output":"done","to_node":"c"}
    ]})
}
fn error(value: Value) -> String {
    validate_definition(&definition(value), &NodeRegistry::from_inventory().unwrap())
        .unwrap_err()
        .to_string()
}
#[test]
fn resolves_instance_ports_and_transitive_references() {
    let mut value = graph();
    value["outputs"] = json!([{"name":"result","node":"c","port":"yes"}]);
    let definition = definition(value);
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    assert_eq!(
        plan.execution_order
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert_eq!(plan.definition.control_edges.len(), 2);
    let mut reordered = definition.clone();
    reordered.control_edges.reverse();
    assert_eq!(plan, compile_definition(&reordered, &registry).unwrap());
}
#[test]
fn factories_run_once_per_preparation() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = graph();
    for node in value["nodes"].as_array_mut().unwrap() {
        node["config"]["count"] = json!(true);
    }
    let plan = plan_definition(&definition(value)).unwrap();
    CONSTRUCTIONS.store(0, Ordering::SeqCst);
    instantiate_compiled(&plan, &registry).unwrap();
    assert_eq!(CONSTRUCTIONS.load(Ordering::SeqCst), 3);
}
#[test]
fn rejects_missing_and_unordered_context_references() {
    for reference in ["missing.value", "a.absent", "c.yes"] {
        let mut value = graph();
        value["nodes"][2]["config"]["reference"] = json!(reference);
        assert!(error(value).contains(reference));
    }
    let mut value = graph();
    value["control_edges"] = json!([]);
    let message = error(value);
    assert!(message.contains("explicit dependency") && message.contains("branch-test"));
    let mut value = graph();
    value["nodes"][0]["config"]["reference"] = json!("c.yes");
    assert!(error(value).contains("explicit dependency"));
}

#[test]
fn validates_sparse_references_in_a_deep_shared_graph() {
    let mut nodes = Vec::new();
    let mut controls = Vec::new();
    for index in 0..4000 {
        let id = format!("n{index:04}");
        nodes.push(json!({"id": id, "kind": "test.dynamic", "config": {"ports": ["value"]}}));
        if index > 0 {
            controls.push(json!({
                "from_node": format!("n{:04}", index - 1), "from_output": "value", "to_node": id
            }));
        }
    }
    nodes.push(json!({"id": "isolated", "kind": "test.dynamic", "config": {"ports": ["value"]}}));
    for index in [2000, 3999] {
        nodes[index]["config"]["reference"] = json!("n0000.value");
    }
    let mut value = json!({"version": "2026-09-26", "dependencies": {}, "nodes": nodes, "control_edges": controls});
    let registry = NodeRegistry::from_inventory().unwrap();
    validate_definition(&definition(value.clone()), &registry).unwrap();
    value["nodes"][3999]["config"]["reference"] = json!("isolated.value");
    assert!(error(value).contains("explicit dependency"));
}
#[test]
fn rejects_port_collisions_and_invalid_descriptors() {
    for ports in [json!([""]), json!(["value", "value"])] {
        let mut value = graph();
        value["nodes"][0]["config"]["ports"] = ports;
        assert!(error(value).contains("port"));
    }
    let mut value = graph();
    value["nodes"][0]["id"] = json!("a.b");
    value["nodes"][0]["config"]["ports"] = json!(["c"]);
    value["nodes"][1]["id"] = json!("a");
    value["nodes"][1]["config"]["ports"] = json!(["b.c"]);
    value["nodes"][2]["config"]
        .as_object_mut()
        .unwrap()
        .remove("reference");
    value["control_edges"] = json!([]);
    assert!(error(value).contains("a.b.c"));
}
#[test]
fn exact_dotted_references_do_not_split_names() {
    let mut value = graph();
    value["nodes"][0]["id"] = json!("a.source");
    value["nodes"][2]["config"]["reference"] = json!("a.source.value");
    value["control_edges"][0]["from_node"] = json!("a.source");
    validate_definition(&definition(value), &NodeRegistry::from_inventory().unwrap()).unwrap();
}
#[test]
fn validates_control_structure_and_union_cycles() {
    let mut value = graph();
    value["control_edges"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from_node":"c","from_output":"yes","to_node":"a"}));
    assert!(error(value).contains("cycle"));
    let mut value = graph();
    let duplicate = value["control_edges"][0].clone();
    value["control_edges"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert!(error(value).contains("duplicate"));
    for field in ["from_node", "to_node", "from_output"] {
        let mut value = graph();
        value["control_edges"][0][field] = json!("unknown");
        assert!(error(value).contains("unknown"));
    }
    let mut value = graph();
    value["nodes"][2]["config"]
        .as_object_mut()
        .unwrap()
        .remove("reference");
    value["control_edges"] = json!([]);
    let parsed = definition(value);
    assert!(
        serde_json::to_value(parsed)
            .unwrap()
            .get("control_edges")
            .is_none()
    );
}

#[test]
fn plans_union_dependencies_once_and_rejects_mixed_cycles() {
    let mut value = graph();
    value["edges"] =
        json!([{"from_node":"a","from_output":"value","to_node":"b","to_input":"input"}]);
    let plan = plan_definition(&definition(value.clone())).unwrap();
    assert_eq!(plan.execution_order.len(), 3);
    value["edges"][0] =
        json!({"from_node":"c","from_output":"yes","to_node":"a","to_input":"input"});
    assert!(
        plan_definition(&definition(value))
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
    let mut invalid_order = plan;
    invalid_order.execution_order.reverse();
    assert!(invalid_order.generate_artifacts().is_err());
}
