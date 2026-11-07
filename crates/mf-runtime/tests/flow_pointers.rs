use mf_runtime::{NodeDefinition, WorkflowDefinition};
use serde_json::{Value, json};

fn flow() -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version": "2026-10-28", "dependencies": {}, "nodes": []
    }))
    .unwrap()
}

fn node(kind: &str, config: Value) -> NodeDefinition {
    serde_json::from_value(json!({"id": "dispatch", "kind": kind, "config": config})).unwrap()
}

#[test]
fn nested_and_escaped_pointers_select_only_explicitly_authorized_locations() {
    let body = flow();
    let config = json!({
        "branches": [{"body": body}],
        "handlers": {"a/b~": body, "": body},
        "ordinary": body
    });
    let mut node = node("user.dispatch", config.clone());
    let paths = ["/branches/0/body", "/handlers/a~1b~0", "/handlers/"];
    let selected = node.flow_parameters(&paths).unwrap();
    assert_eq!(selected.len(), 3);
    assert!(selected.values().all(|value| value == &body));
    assert!(!selected.contains_key("/ordinary"));
    let id = node.id.clone();
    for path in paths {
        node.set_node_flow(path, body.clone()).unwrap();
        assert_eq!(node.id, id);
        assert_eq!(node.config, config);
    }
    let encoded = serde_json::to_value(&node).unwrap();
    let decoded: NodeDefinition = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.flow_parameters(&paths).unwrap(), selected);
}

#[test]
fn root_pointer_is_a_real_flow_location_and_setter_keeps_node_identity() {
    let body = flow();
    let mut node = node("user.root", json!(body));
    assert_eq!(node.flow_parameters(&[""]).unwrap()[""], body);
    let id = node.id.clone();
    node.set_node_flow("", body.clone()).unwrap();
    assert_eq!(node.id, id);
    assert_eq!(node.config, json!(body));
}

#[test]
fn root_empty_key_escapes_and_duplicate_declarations_remain_distinct() {
    let body = flow();
    let mut node = node("user.dispatch", json!({"": body, "~1": body, "/": body}));
    for path in ["", "/", "/~01", "/~1", "/a~0b~1c"] {
        NodeDefinition::validate_flow_parameter_path(path).unwrap();
    }
    for path in ["body", "/~", "/~2", "/a~10~"] {
        assert!(
            NodeDefinition::validate_flow_parameter_path(path).is_err(),
            "{path}"
        );
    }
    assert!(node.flow_parameters(&[""]).is_err()); // The enclosing object is not a Flow.
    assert_eq!(
        node.flow_parameters(&["/", "/~01", "/~1"]).unwrap().len(),
        3
    );
    assert!(node.flow_parameters(&["/", "/"]).is_err());
    let original = node.clone();
    node.set_node_flow("/", body.clone()).unwrap();
    assert_eq!(node, original);
    node.set_node_flow("", body.clone()).unwrap();
    assert_eq!(node.config, json!(body));
}

#[test]
fn setters_never_fabricate_intermediate_parents_or_append_array_entries() {
    let original = node(
        "user.dispatch",
        json!({
            "branches": [{"body": null}], "handlers": {}, "scalar": 42
        }),
    );
    let before = original.config.clone();
    for path in [
        "",
        "/scalar",
        "/branches/0",
        "/branches/0/body",
        "/handlers/a~1b~0",
        "/handlers/",
    ] {
        original.validate_flow_parameter_target(path).unwrap();
        assert_eq!(original.config, before, "{path}");
    }
    for path in [
        "body",
        "/missing/body",
        "/handlers/missing/body",
        "/scalar/body",
        "/branches/1",
        "/branches/1/body",
        "/branches/-",
        "/branches/01/body",
        "/branches/0/body/child",
        "/invalid~2",
    ] {
        assert!(
            original.validate_flow_parameter_target(path).is_err(),
            "{path}"
        );
        assert_eq!(original.config, before, "{path}");
        let mut changed = original.clone();
        assert!(changed.set_node_flow(path, flow()).is_err(), "{path}");
        assert_eq!(changed, original, "{path}");
    }
    let mut changed = original;
    changed.set_node_flow("/handlers/a~1b~0", flow()).unwrap();
    assert_eq!(changed.config["handlers"]["a/b~"], json!(flow()));
    changed.set_node_flow("/branches/0/body", flow()).unwrap();
    assert_eq!(changed.config["branches"][0]["body"], json!(flow()));
}
