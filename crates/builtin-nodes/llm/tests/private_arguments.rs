extern crate mfn_llm as _;

use mf_runtime::{ExecutionContext, Inputs, NodeRegistry, ValueRef};
use serde_json::json;

#[test]
fn registered_private_argument_merge_preserves_isolation_and_checks_its_contract() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let registration = registry.get("llm.private_arguments").unwrap();
    let prepared = registration.instantiate(json!({})).unwrap();
    assert!(prepared.metadata.input_bindings["bindings"].secret);
    let task = prepared.execution.as_task_node().unwrap();
    let arguments: ValueRef = json!({"query":"example"}).into();
    let bindings = ValueRef::from(json!({"private":{"api_key":"synthetic-key"}})).into_secret();
    for name in ["private", "public"] {
        let result = task
            .execute(
                Inputs::from([
                    ("name".into(), name.into()),
                    ("arguments".into(), arguments.clone()),
                    ("bindings".into(), bindings.clone()),
                ]),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        let merged = &result.outputs["arguments"];
        if name == "private" {
            assert_eq!(merged["api_key"].as_str(), Some("synthetic-key"));
            assert!(merged.contains_secret());
        } else {
            assert!(merged.ptr_eq(&arguments));
            assert!(!merged.contains_secret());
        }
    }
    for invalid in [
        json!({"name":42,"arguments":{},"bindings":{}}),
        json!({"name":"public","arguments":[],"bindings":{}}),
        json!({"name":"public","arguments":{},"bindings":42}),
        json!({"name":"public","arguments":{},"bindings":{},"unexpected":true}),
    ] {
        let inputs = invalid
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone().into()))
            .collect();
        assert!(
            task.execute(inputs, &mut ExecutionContext::default())
                .is_err()
        );
    }
    assert!(
        registration
            .instantiate(json!({"unexpected":true}))
            .is_err()
    );
}
