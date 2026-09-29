extern crate mfn_code as _;
extern crate mfn_core as _;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, WorkflowDefinitionVersion, compile_definition,
    plan_definition,
};
use serde_json::{Value, json};

fn definition() -> Value {
    json!({
        "version": "2026-09-29",
        "dependencies": {},
        "nodes": [
            {"id": "seed", "kind": "builtin.constant", "config": {"value": 0}},
            {
                "id": "repeat",
                "kind": "workflow.loop",
                "loop": {
                    "max_iterations": 5,
                    "variables": [{"name": "count", "type": "int"}],
                    "until": {"variable": "count", "operator": "gte", "value": 3},
                    "body": {
                        "nodes": [
                            {"id": "assign", "kind": "workflow.loop_assign", "config": {"variable": "count"}},
                            {"id": "increment", "kind": "builtin.code", "config": {
                                "language": "cel", "inputs": {"count": "int"},
                                "code": {"next": "count + 1"}
                            }}
                        ],
                        "edges": [
                            {"from_node": "$loop", "from_output": "count", "to_node": "increment", "to_input": "count"},
                            {"from_node": "increment", "from_output": "next", "to_node": "assign", "to_input": "value"}
                        ]
                    }
                }
            }
        ],
        "edges": [{"from_node": "seed", "from_output": "value", "to_node": "repeat", "to_input": "count"}],
        "outputs": [{"name": "count", "node": "repeat", "port": "count"}]
    })
}

fn parse(value: Value) -> WorkflowDefinition {
    serde_json::from_value(value).unwrap()
}

#[test]
fn plans_and_type_checks_nested_loop_body() {
    let definition = parse(definition());
    assert_eq!(definition.version, WorkflowDefinitionVersion::CURRENT);
    let plan = plan_definition(&definition).unwrap();
    let body = &plan.definition.nodes[1]
        .loop_definition
        .as_ref()
        .unwrap()
        .body;
    assert_eq!(body.nodes[0].id.as_str(), "increment");
    assert_eq!(body.nodes[1].id.as_str(), "assign");
    compile_definition(&definition, &NodeRegistry::from_inventory().unwrap()).unwrap();
}

#[test]
fn rejects_old_schema_and_invalid_loop_structure() {
    let mut value = definition();
    value["version"] = json!("2026-09-26");
    assert!(
        plan_definition(&parse(value))
            .unwrap_err()
            .to_string()
            .contains("2026-09-29")
    );

    let mut value = definition();
    value["nodes"][1]["loop"]["max_iterations"] = json!(0);
    assert!(
        plan_definition(&parse(value))
            .unwrap_err()
            .to_string()
            .contains("max_iterations")
    );

    let mut value = definition();
    value["nodes"][1]["loop"]["body"]["edges"][0]["from_node"] = json!("seed");
    assert!(
        plan_definition(&parse(value))
            .unwrap_err()
            .to_string()
            .contains("unknown source")
    );

    let mut value = definition();
    value["nodes"][1]["loop"]["body"]["control_edges"] = json!([{
        "from_node": "assign", "from_output": "done", "to_node": "increment"
    }]);
    assert!(
        plan_definition(&parse(value))
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
}

#[test]
fn validates_body_plugins_and_loop_port_types() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = definition();
    value["nodes"][1]["loop"]["body"]["nodes"][1]["kind"] = json!("missing.plugin");
    assert!(
        compile_definition(&parse(value), &registry)
            .unwrap_err()
            .to_string()
            .contains("missing.plugin")
    );

    let mut value = definition();
    value["nodes"][1]["loop"]["variables"][0]["type"] = json!("string");
    value["nodes"][1]["loop"]["until"] = Value::Null;
    let error = compile_definition(&parse(value), &registry)
        .unwrap_err()
        .to_string();
    assert!(error.contains("cannot connect"), "{error}");

    let mut value = definition();
    value["nodes"][1]["loop"]["body"]["nodes"][0]["config"]["variable"] = json!("unknown");
    assert!(
        plan_definition(&parse(value))
            .unwrap_err()
            .to_string()
            .contains("unknown variable")
    );
}

#[test]
fn initial_literal_does_not_specialize_later_passes() {
    let mut value = definition();
    value["nodes"][0]["config"]["value"] = json!("initial");
    value["nodes"][1]["loop"]["variables"][0]["type"] = json!("any");
    value["nodes"][1]["loop"]["until"] = Value::Null;
    compile_definition(&parse(value), &NodeRegistry::from_inventory().unwrap()).unwrap();
}
