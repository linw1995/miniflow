use mf_runtime::{
    CallConfig, CompiledGraph, IterationConfig, LoopDefinition, NodeDefinition, NodeFactory,
    NodeRegistration, WorkflowDefinition, WorkflowDefinitionVersion,
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
        "/selected"
    } else {
        "/body"
    }
}

fn extract(
    config: &Value,
    name: &str,
) -> Result<std::collections::BTreeMap<String, Value>, mf_runtime::NodeBuildError> {
    Ok([(
        name.into(),
        config.pointer(name).cloned().unwrap_or(Value::Null),
    )]
    .into())
}

fn receiver(node: &NodeDefinition) -> NodeRegistration {
    NodeRegistration {
        kind: "test.child",
        factory: NodeFactory::Subgraph {
            flow_parameters: match node.kind.as_str() {
                "workflow.call" | "workflow.loop" | "builtin.iteration" => {
                    |config| extract(config, "/body")
                }
                "user.dispatch" => |config| extract(config, "/selected"),
                kind => panic!("unknown fixture kind: {kind}"),
            },
            plain: |_| panic!("no providers needed"),
            prepared: |_, _, _| panic!("no providers needed"),
        },
    }
}

#[test]
fn declared_children_keep_explicit_versions_and_independent_dependencies() {
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
            receiver(&node)
                .validate_flow_parameters(&node, mf_runtime::WorkflowDefinitionVersion::CURRENT)
                .unwrap(),
            flows
        );
        assert_eq!(flows.len(), 1);
        let body = &flows[name];
        assert_eq!(body.version, WorkflowDefinitionVersion::V2026_10_25);
        assert!(body.dependencies.is_empty());
        assert!(body.nodes.is_empty());
        let mut canonical = node.config.pointer(name).unwrap().clone();
        canonical.as_object_mut().unwrap().remove("dependencies");
        assert_eq!(json!(body), canonical);
        let child = CompiledGraph {
            definition: body.clone(),
            execution_order: Vec::new(),
            relations: Default::default(),
            children: Default::default(),
        };
        assert_eq!(
            child.child_scope_key(&node, name),
            mf_telemetry::event::ScopePathEntry::binding_key(node.id.as_str(), name)
        );
        assert_ne!(child.child_scope_key(&node, "other"), node.id.to_string());
        assert_eq!(
            receiver(&node).flow_parameters(&node.config).unwrap(),
            [(name.into(), node.config.pointer(name).unwrap().clone())].into()
        );
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
        assert_eq!(node.config.pointer(name).unwrap(), &json!(body));
        for (key, value) in original.as_object().unwrap() {
            if key != name.trim_start_matches('/') {
                assert_eq!(&node.config[key], value);
            }
        }
        assert!(node.config.get("definition").is_none());
        assert!(node.config.pointer(name).unwrap().get("result").is_none());
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
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    let children = parent
        .nodes
        .iter()
        .map(|node| {
            let bodies = receiver(node)
                .validate_flow_parameters(node, parent.version)
                .unwrap()
                .into_iter()
                .map(|(name, body)| {
                    let graph = CompiledGraph {
                        execution_order: body.nodes.iter().map(|node| node.id.clone()).collect(),
                        definition: body,
                        relations: Default::default(),
                        children: Default::default(),
                    };
                    (name, graph)
                })
                .collect();
            (node.id.clone(), bodies)
        })
        .collect();
    let graph = CompiledGraph {
        definition: parent,
        execution_order: order,
        relations: Default::default(),
        children,
    };
    let description = mf_runtime::describe_graph(&graph).unwrap();
    assert_eq!(description.loop_bodies.len(), 4);
    for child in &description.loop_bodies {
        match child.path[0].as_str() {
            key if key == mf_telemetry::event::ScopePathEntry::binding_key("loop", "/body") => {
                assert_eq!(child.nodes.len(), 1);
                assert_eq!(child.nodes[0].id, "%loop");
                assert_eq!(child.nodes[0].kind, "%loop");
            }
            key if key
                == mf_telemetry::event::ScopePathEntry::binding_key("iteration", "/body") =>
            {
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
        assert!(node.flow_parameters(&["/body"]).unwrap().is_empty());
        assert!(
            receiver(&node)
                .validate_flow_parameters(&node, mf_runtime::WorkflowDefinitionVersion::CURRENT)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn invalid_field_names_leave_existing_configuration_unchanged() {
    let body = definition("2026-10-28");
    let original = nodes().pop().unwrap();
    for name in ["body", "/~", "/~2"] {
        let mut node = original.clone();
        let error = node.set_node_flow(name, body.clone()).unwrap_err();
        assert!(
            matches!(error, mf_runtime::FlowParameterError::Invalid { message }
            if message.contains("JSON pointer"))
        );
        assert_eq!(node, original);
        assert!(node.flow_parameters(&[name]).is_err());
        assert_eq!(node.flow_parameters(&["/selected"]).unwrap().len(), 1);
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
        assert!(node.flow_parameters(&["/selected"]).unwrap().is_empty());
        let error = node
            .set_node_flow("/selected", definition("2026-10-28"))
            .unwrap_err();
        assert!(
            matches!(error, mf_runtime::FlowParameterError::Invalid { message }
            if message == "Flow parameter path has a missing parent or array element")
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
            *node.config.pointer_mut(name).unwrap() = value;
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
        assert!(matches!(node.flow_parameters(&["/selected"]).unwrap_err(),
            mf_runtime::FlowParameterError::Json { parameter, source }
                if parameter == "/selected" && source.is_data()));
        assert_eq!(node, original);
    }
}

#[test]
fn definition_is_still_an_ordinary_custom_field_not_a_global_alias_for_body() {
    let mut node = nodes().pop().unwrap();
    let child = definition("2026-10-25");
    node.config = json!({"definition": child, "path": "child.json"});
    assert!(node.flow_parameters(&["/body"]).unwrap().is_empty());
    assert_eq!(
        node.flow_parameters(&["/definition"]).unwrap()["/definition"],
        child
    );
    node.set_node_flow("/selected", definition("2026-10-28"))
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
    for field in ["version", "nodes"] {
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
        assert_eq!(
            node.config.pointer(name).unwrap()["edges"][0]["to_input"],
            "value"
        );
    }
    let modern_body: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [{"id": "source", "kind": "user.source"}, {"id": "sink", "kind": "user.sink"}],
        "edges": [{"from_node": "source", "to_node": "sink"}]
    }))
    .unwrap();
    assert!(modern_body.has_order_edges());
    parent.nodes[0].set_node_flow("/body", modern_body).unwrap();
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
