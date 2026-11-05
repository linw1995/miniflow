use mf_runtime::{
    CallConfig, ConfigParameterType, ConfigParameterTypes, IterationConfig, LoopDefinition,
    NodeDefinition, NodeFactory, NodeRegistration, WorkflowDefinition, WorkflowDefinitionVersion,
};
use serde_json::{Value, json};
use std::error::Error;

fn definition(version: &str) -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version": version, "dependencies": {}, "nodes": []
    }))
    .unwrap()
}

fn nodes() -> Vec<NodeDefinition> {
    let child = definition("2026-10-25");
    serde_json::from_value(json!([
        {"id": "call", "kind": "workflow.call", "config": {
            "flow": "child", "body": child
        }},
        {"id": "loop", "kind": "workflow.loop", "config": {
            "max_iterations": 3, "variables": [], "body": child
        }},
        {"id": "iteration", "kind": "builtin.iteration", "config": {
            "mode": "parallel", "on_error": "continue_on_error", "body": {
                "version": "2026-10-25", "dependencies": {}, "nodes": [], "edges": [],
                "outputs": [{"name": "result", "node": "%iteration", "port": "item"}]
            }
        }},
        {"id": "custom", "kind": "user.dispatch", "config": {
            "selected": child, "option": 42,
            "nested": {"ignored": child}
        }}
    ]))
    .unwrap()
}

fn parameter(node: &NodeDefinition) -> &'static str {
    if node.kind == "user.dispatch" {
        "selected"
    } else {
        "body"
    }
}

fn parameter_types(kind: &str) -> ConfigParameterTypes {
    use ConfigParameterType::{Flow, Optional, Value};
    match kind {
        "workflow.call" => [
            ("body".into(), Optional(Box::new(Flow))),
            ("flow".into(), Value),
        ]
        .into(),
        "workflow.loop" => [
            ("body".into(), Flow),
            ("max_iterations".into(), Value),
            ("variables".into(), Value),
        ]
        .into(),
        "builtin.iteration" => [
            ("body".into(), Flow),
            ("mode".into(), Value),
            ("on_error".into(), Value),
        ]
        .into(),
        "user.dispatch" => [
            ("selected".into(), Flow),
            ("option".into(), Value),
            ("nested".into(), Value),
        ]
        .into(),
        "%loop" | "builtin.iteration_input" => Default::default(),
        _ => panic!("unknown fixture kind: {kind}"),
    }
}

#[test]
fn all_child_kinds_use_declared_fields_without_inheriting_parent_schema_or_dependencies() {
    let mut parent = definition("2026-10-28");
    parent.dependencies = serde_json::from_value(json!({
        "provider": {"package": "provider", "path": "../provider"}
    }))
    .unwrap();
    parent.nodes = nodes();
    let decoded: WorkflowDefinition = serde_json::from_value(json!(parent)).unwrap();
    for node in decoded.nodes {
        let name = parameter(&node);
        assert!(node.flow_parameters(&[]).unwrap().is_empty());
        let flows = node.flow_parameters(&[name]).unwrap();
        assert_eq!(
            node.declared_flows(&[(node.kind.clone(), parameter_types(&node.kind))].into())
                .unwrap(),
            flows
        );
        assert_eq!(flows.len(), 1);
        let body = &flows[name];
        assert_eq!(body.version, WorkflowDefinitionVersion::V2026_10_25);
        assert!(body.dependencies.is_empty());
        assert!(body.nodes.is_empty());
        assert_eq!(json!(body), node.config[name]);
        if name == "body" {
            assert_eq!(node.flow_scope_key(name), node.id.to_string());
        } else {
            assert_eq!(
                node.flow_scope_key(name),
                mf_telemetry::event::ScopePathEntry::binding_key("custom", name)
            );
        }
        assert_ne!(node.flow_scope_key("other"), node.id.to_string());
        NodeRegistration {
            kind: "test.child",
            factory: NodeFactory::Subgraph {
                parameter_types: match node.kind.as_str() {
                    "workflow.call" => || parameter_types("workflow.call"),
                    "workflow.loop" => || parameter_types("workflow.loop"),
                    "builtin.iteration" => || parameter_types("builtin.iteration"),
                    "user.dispatch" => || parameter_types("user.dispatch"),
                    _ => unreachable!(),
                },
                plain: |_| panic!("no providers needed"),
                prepared: |_, _, _| panic!("no providers needed"),
            },
        }
        .validate_flow_parameters(&node)
        .unwrap();
    }
}

#[test]
fn setters_preserve_full_definitions_and_unrelated_configuration_for_every_kind() {
    let mut parent = definition("2026-10-28");
    for mut node in nodes() {
        let name = parameter(&node);
        if matches!(node.kind.as_str(), "workflow.loop" | "builtin.iteration") {
            node.config["stream_outputs"] =
                json!({"events": {"node": "normalized", "port": "value"}});
        }
        let original = node.config.clone();
        let mut body = definition("2026-10-28");
        body.execution = Some(Default::default());
        body.nodes.push(
            serde_json::from_value(json!({"id": "normalized", "kind": "user.noop"})).unwrap(),
        );
        // The setter is generic: iteration projection and Loop output semantics
        // belong to the compiler/provider, not to a format back-conversion shim.
        body.outputs = serde_json::from_value(json!([
            {"name": "extra", "node": "normalized", "port": "value", "optional": true}
        ]))
        .unwrap();
        node.set_node_flow(name, body.clone()).unwrap();
        assert_eq!(node.flow_parameters(&[name]).unwrap()[name], body);
        assert_eq!(node.config[name], json!(body));
        for (key, value) in original.as_object().unwrap() {
            if key != name {
                assert_eq!(&node.config[key], value);
            }
        }
        assert!(node.config.get("definition").is_none());
        assert!(node.config[name].get("result").is_none());
        parent.nodes.push(node);
    }
    let round_trip: WorkflowDefinition =
        serde_json::from_value(serde_json::to_value(&parent).unwrap()).unwrap();
    assert_eq!(round_trip, parent);
}

#[test]
fn manifest_projects_explicit_compiled_sources_with_complete_child_orders() {
    let mut parent = definition("2026-10-28");
    parent.nodes = nodes();
    for node in &mut parent.nodes {
        let source = match node.kind.as_str() {
            "workflow.loop" => json!({"id": "%loop", "kind": "%loop", "config": {}}),
            "builtin.iteration" => json!({"id": "%iteration", "kind": "builtin.iteration_input"}),
            _ => continue,
        };
        node.config["body"]["nodes"] = json!([source]);
    }
    let order = parent
        .nodes
        .iter()
        .map(|node| node.id.to_string())
        .collect::<Vec<_>>();
    let child_orders = parent
        .nodes
        .iter()
        .map(|node| {
            let name = parameter(node);
            let body = node.flow_parameters(&[name]).unwrap().remove(name).unwrap();
            mf_runtime::LoopExecutionOrder {
                path: vec![node.flow_scope_key(name)],
                execution_order: body.nodes.iter().map(|node| node.id.to_string()).collect(),
            }
        })
        .collect::<Vec<_>>();
    let declarations = [
        "workflow.call",
        "workflow.loop",
        "builtin.iteration",
        "user.dispatch",
        "%loop",
        "builtin.iteration_input",
    ]
    .into_iter()
    .map(|kind| (kind.into(), parameter_types(kind)))
    .collect();
    let description =
        mf_runtime::describe_definition(&parent, &order, &child_orders, &declarations).unwrap();
    assert_eq!(description.loop_bodies.len(), 4);
    for child in &description.loop_bodies {
        match child.path[0].as_str() {
            "loop" => {
                assert_eq!(child.nodes.len(), 1);
                assert_eq!(child.nodes[0].id, "%loop");
                assert_eq!(child.nodes[0].kind, "%loop");
            }
            "iteration" => {
                assert_eq!(child.nodes.len(), 1);
                assert_eq!(child.nodes[0].id, "%iteration");
                assert_eq!(child.nodes[0].kind, "builtin.iteration_input");
            }
            _ => assert!(child.nodes.is_empty()),
        }
    }
}

#[test]
fn missing_and_null_fields_are_optional_and_unresolved_calls_are_not_special_cased() {
    for config in [json!({}), json!({"body": null}), json!({"flow": "missing"})] {
        let node: NodeDefinition = serde_json::from_value(json!({
            "id": "call", "kind": "workflow.call", "config": config
        }))
        .unwrap();
        assert!(node.flow_parameters(&["body"]).unwrap().is_empty());
        assert!(
            node.declared_flows(
                &[("workflow.call".into(), parameter_types("workflow.call"))].into()
            )
            .unwrap()
            .is_empty()
        );
    }
}

#[test]
fn invalid_field_names_leave_existing_configuration_unchanged() {
    let body = definition("2026-10-28");
    let original = nodes().pop().unwrap();
    for name in ["", " ", "\t\n"] {
        let mut node = original.clone();
        let error = node.set_node_flow(name, body.clone()).unwrap_err();
        assert!(
            matches!(error, mf_runtime::FlowParameterError::Invalid { message }
            if message == "Flow parameter field names must be nonblank")
        );
        assert_eq!(node, original);
        assert!(node.flow_parameters(&[name]).is_err());
        assert_eq!(node.flow_parameters(&["selected"]).unwrap().len(), 1);
    }
}

#[test]
fn nonobject_provider_configs_are_opaque_but_cannot_receive_flow_fields() {
    for config in [
        Value::Null,
        json!(false),
        json!(42),
        json!("opaque"),
        json!([]),
    ] {
        let mut node = nodes().pop().unwrap();
        node.config = config;
        let original = node.clone();
        assert!(node.flow_parameters(&["selected"]).unwrap().is_empty());
        let error = node
            .set_node_flow("selected", definition("2026-10-28"))
            .unwrap_err();
        assert!(
            matches!(error, mf_runtime::FlowParameterError::Invalid { message }
            if message == "Flow parameters require an object configuration")
        );
        assert_eq!(node, original);
    }
}

#[test]
fn malformed_declared_fields_preserve_json_sources_without_mutation() {
    for mut node in nodes() {
        let name = parameter(&node);
        for value in [
            json!({}),
            json!({"nodes": []}),
            json!(42),
            json!({"version": "2099-01-01"}),
        ] {
            node.config[name] = value;
            let original = node.clone();
            let error = node.flow_parameters(&[name]).unwrap_err();
            assert!(
                matches!(&error, mf_runtime::FlowParameterError::Json { parameter, source }
                    if parameter == name && source.is_data())
            );
            assert!(
                error
                    .source()
                    .unwrap()
                    .downcast_ref::<serde_json::Error>()
                    .is_some()
            );
            assert_eq!(node, original);
        }
    }
}

#[test]
fn unencodable_children_do_not_partially_replace_configuration_fields() {
    let mut body = definition("2026-10-28");
    body.edges.push(
        serde_json::from_value(json!({
            "from_node": "source", "from_output": "value", "to_node": "missing", "to_input": "value"
        }))
        .unwrap(),
    );
    for mut node in nodes() {
        let name = parameter(&node);
        let original = node.clone();
        let error = node.set_node_flow(name, body.clone()).unwrap_err();
        assert!(
            matches!(&error, mf_runtime::FlowParameterError::Json { parameter, source }
            if parameter == name && source.to_string().contains("data binding targets an unknown node"))
        );
        assert!(
            error
                .source()
                .unwrap()
                .downcast_ref::<serde_json::Error>()
                .is_some()
        );
        assert_eq!(node, original);
    }
}

#[test]
fn definition_shaped_json_is_opaque_without_field_authority() {
    for value in [
        json!({"definition": definition("2026-10-28")}),
        json!({"path": "child.json"}),
    ] {
        let mut node = nodes().pop().unwrap();
        node.config["selected"] = value;
        let original = node.clone();
        assert!(node.flow_parameters(&[]).unwrap().is_empty());
        let mut parent = definition("2026-10-28");
        parent.nodes.push(node.clone());
        assert_eq!(
            serde_json::from_value::<WorkflowDefinition>(json!(parent)).unwrap(),
            parent
        );
        assert!(matches!(node.flow_parameters(&["selected"]).unwrap_err(),
            mf_runtime::FlowParameterError::Json { parameter, source }
                if parameter == "selected" && source.is_data()));
        assert_eq!(node, original);
    }
}

#[test]
fn definition_is_still_an_ordinary_custom_field_not_a_global_alias_for_body() {
    let mut node = nodes().pop().unwrap();
    let child = definition("2026-10-25");
    node.config = json!({"definition": child, "path": "child.json"});
    assert!(node.flow_parameters(&["body"]).unwrap().is_empty());
    assert_eq!(
        node.flow_parameters(&["definition"]).unwrap()["definition"],
        child
    );
    node.set_node_flow("selected", definition("2026-10-28"))
        .unwrap();
    assert_eq!(node.config["definition"], json!(child));
    assert_eq!(node.config["path"], "child.json");
    assert!(node.config.get("body").is_none());
}

#[test]
fn old_top_level_loop_and_incomplete_definitions_are_rejected() {
    assert!(
        serde_json::from_value::<NodeDefinition>(json!({
            "id": "loop", "kind": "workflow.loop", "loop": {
                "max_iterations": 3, "variables": [], "body": {"nodes": []}
            }
        }))
        .is_err()
    );
    for field in ["version", "dependencies"] {
        let mut body = json!(definition("2026-10-25"));
        body.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<WorkflowDefinition>(body).is_err());
    }
}

#[test]
fn typed_builtin_config_validation_rejects_legacy_malformed_and_null_required_fields() {
    fn reject<T: serde::de::DeserializeOwned>(values: impl IntoIterator<Item = Value>) {
        for value in values {
            let Err(error) = serde_json::from_value::<T>(value.clone()) else {
                panic!("accepted malformed config: {value}");
            };
            assert!(error.is_data(), "{value}: {error}");
        }
    }
    reject::<CallConfig>([
        Value::Null,
        json!({}),
        json!({"flow": 42}),
        json!({"flow": "child", "body": {"nodes": []}}),
        json!({"flow": "child", "definition": definition("2026-10-25")}),
    ]);
    reject::<LoopDefinition>([
        Value::Null,
        json!({}),
        json!({"max_iterations": "three", "variables": [], "body": definition("2026-10-25")}),
        json!({"max_iterations": 3, "variables": [], "body": null}),
        json!({"max_iterations": 3, "variables": [], "body": {"nodes": []}}),
    ]);
    reject::<IterationConfig>([
        Value::Null,
        json!({}),
        json!({"body": null}),
        json!({"body": definition("2026-10-25"), "mode": "unknown"}),
        json!({"body": definition("2026-10-25"), "on_error": "unknown"}),
        json!({"body": {"nodes": [], "result": {"node": "%iteration", "port": "item"}}}),
    ]);
}

#[test]
fn typed_builtin_configs_accept_the_same_direct_body_field() {
    let mut nodes = nodes();
    let call: CallConfig = serde_json::from_value(nodes.remove(0).config).unwrap();
    assert_eq!(
        call.body.unwrap().version,
        WorkflowDefinitionVersion::V2026_10_25
    );
    let loop_config: LoopDefinition = serde_json::from_value(nodes.remove(0).config).unwrap();
    assert_eq!(loop_config.max_iterations, 3);
    assert!(loop_config.body.nodes.is_empty());
    let iteration: IterationConfig = serde_json::from_value(nodes.remove(0).config).unwrap();
    assert_eq!(iteration.body.outputs[0].name, "result");
    assert_eq!(iteration.body.outputs[0].port, "item");
}

#[test]
fn parent_and_child_edge_formats_are_decoded_independently() {
    let old_body: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-09-29", "dependencies": {},
        "nodes": [{"id": "source", "kind": "user.source"}, {"id": "sink", "kind": "user.sink"}],
        "edges": [{"from_node": "source", "from_output": "value", "to_node": "sink", "to_input": "value"}]
    })).unwrap();
    let mut parent = definition("2026-10-28");
    for mut node in nodes() {
        let name = parameter(&node);
        node.set_node_flow(name, old_body.clone()).unwrap();
        parent.nodes.push(node);
    }
    let decoded: WorkflowDefinition = serde_json::from_value(json!(parent)).unwrap();
    for node in &decoded.nodes {
        let name = parameter(node);
        assert_eq!(node.flow_parameters(&[name]).unwrap()[name], old_body);
        assert_eq!(node.config[name]["edges"][0]["to_input"], "value");
    }
    let modern_body: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [{"id": "source", "kind": "user.source"}, {"id": "sink", "kind": "user.sink"}],
        "edges": [{"from_node": "source", "to_node": "sink"}]
    }))
    .unwrap();
    assert!(modern_body.has_order_edges());
    parent.nodes[0].set_node_flow("body", modern_body).unwrap();
    assert!(!parent.has_order_edges());
    assert_eq!(
        serde_json::from_value::<WorkflowDefinition>(json!(parent)).unwrap(),
        parent
    );
}

#[test]
fn unsupported_parent_schemas_still_reject_resolved_calls() {
    for version in ["2026-09-26", "2026-10-03", "2026-10-23"] {
        let value = json!({"version": version, "dependencies": {}, "nodes": [nodes().remove(0)]});
        let error = serde_json::from_value::<WorkflowDefinition>(value).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("nested flows require version 2026-10-25")
        );
        assert!(error.is_data());
    }
}

#[test]
fn current_schema_parsing_rejects_unintroduced_and_invalid_versions() {
    assert_eq!(
        WorkflowDefinitionVersion::CURRENT,
        WorkflowDefinitionVersion::V2026_10_28
    );
    let current = definition("2026-10-28");
    assert_eq!(
        WorkflowDefinition::from_json(&serde_json::to_string(&current).unwrap()).unwrap(),
        current
    );
    for version in [
        json!("2026-11-01"),
        json!("2026-11-03"),
        json!("2026-11-04"),
        Value::Null,
        json!(42),
    ] {
        let input = json!({"version": version, "dependencies": {}, "nodes": []}).to_string();
        let error = WorkflowDefinition::from_json(&input).unwrap_err();
        assert!(
            matches!(&error, mf_runtime::DefinitionParseError::JsonParse { source } if source.is_data())
        );
        assert!(
            error
                .source()
                .unwrap()
                .downcast_ref::<serde_json::Error>()
                .is_some()
        );
    }
}
