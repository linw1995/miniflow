extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use serde_json::{Value, json};

fn definition(selected: bool, left: Value, right: Value) -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version":"2026-10-03", "dependencies":{},
        "nodes":[
            {"id":"condition", "kind":"builtin.constant", "config":{"value":selected}},
            {"id":"route", "kind":"builtin.if_else", "config":{"branches":[{"id":"left", "condition":{"source":{"output":"condition.value", "path":""}, "operator":"eq", "value":true}}]}},
            {"id":"left", "kind":"builtin.constant", "config":{"value":left}},
            {"id":"right", "kind":"builtin.constant", "config":{"value":right}},
            {"id":"consume", "kind":"builtin.identity"}
        ],
        "control_edges":[
            {"from_node":"condition", "from_output":"value", "to_node":"route"},
            {"from_node":"route", "from_output":"left", "to_node":"left"},
            {"from_node":"route", "from_output":"else", "to_node":"right"}
        ],
        "exclusive_inputs":[{"to_node":"consume", "to_input":"input", "sources":[
            {"from_node":"left", "from_output":"value"}, {"from_node":"right", "from_output":"value"}
        ]}],
        "outputs":[{"name":"result", "node":"consume", "port":"value", "optional":true}]
    })).unwrap()
}

#[test]
fn converges_both_branches_without_arbitrary_type_or_constant_inference() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (left, right) in [
        (json!(1), json!(2)),
        (json!(false), json!(null)),
        (json!([1]), json!({"text":"two"})),
    ] {
        for selected in [true, false] {
            let definition = definition(selected, left.clone(), right.clone());
            let plan = compile_definition(&definition, &registry).unwrap();
            let flow = instantiate_compiled(&plan, &registry).unwrap();
            let result = flow.execute().unwrap();
            assert_eq!(
                result["result"],
                if selected {
                    left.clone()
                } else {
                    right.clone()
                }
            );
        }
    }
}

#[test]
fn ordinary_gates_remain_mandatory_and_ambiguity_is_not_hidden() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut definition = definition(true, json!(1), json!(1));
    definition.control_edges.push(
        serde_json::from_value(
            json!({"from_node":"route", "from_output":"else", "to_node":"consume"}),
        )
        .unwrap(),
    );
    let plan = compile_definition(&definition, &registry).unwrap();
    assert!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()
            .is_empty()
    );
    definition
        .control_edges
        .retain(|edge| edge.to_node.as_str() != "left" && edge.to_node.as_str() != "right");
    let plan = compile_definition(&definition, &registry).unwrap();
    let error = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap_err();
    assert!(error.to_string().contains("multiple produced candidates"));
}
