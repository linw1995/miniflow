use mf_runtime::{
    CompiledGraph, DefinitionId, MANIFEST_FRAMING_VERSION, MANIFEST_HEADER_BYTES, MANIFEST_MAGIC,
    MAX_MANIFEST_PADDING_BYTES, MAX_MANIFEST_PAYLOAD_BYTES, ScopeRelations, ValueType,
    WorkflowInput, WorkflowInputSchema, WorkflowInterfaceVersion, WorkflowManifest,
    WorkflowManifestError, WorkflowManifestVersion,
};
use mf_telemetry::identity::WorkflowId;
use serde_json::{Value, json};
use std::error::Error;

fn graph(definition: Value, order: &[&str]) -> CompiledGraph {
    CompiledGraph {
        definition: serde_json::from_value(definition).unwrap(),
        execution_order: order.iter().map(|id| (*id).into()).collect(),
        relations: Default::default(),
        children: Default::default(),
    }
}

fn child<'a>(graph: &'a mut CompiledGraph, owner: &str, binding: &str) -> &'a mut CompiledGraph {
    graph
        .children
        .get_mut(&DefinitionId::from(owner))
        .unwrap()
        .get_mut(binding)
        .unwrap()
}

fn bind(graph: &mut CompiledGraph, owner: &str, binding: &str, body: CompiledGraph) {
    graph
        .children
        .entry(owner.into())
        .or_default()
        .insert(binding.into(), body);
}

fn round_trip(manifest: &WorkflowManifest) -> WorkflowManifest {
    let decoded = WorkflowManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap();
    assert_eq!(&decoded, manifest);
    decoded
}

// Both the writer and an offline reader must reject inconsistent compiled facts.
fn errors(manifest: &WorkflowManifest) -> [WorkflowManifestError; 2] {
    [
        manifest.to_bytes().unwrap_err(),
        WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(manifest).unwrap())).unwrap_err(),
    ]
}

#[test]
fn compiled_manifests_derive_views_from_original_facts() {
    let config = json!({
        "model": {"$ref": "parameters.model"}, "temperature": 0.25,
        "options": {"labels": ["first", "second"]}
    });
    let manifest = WorkflowManifest {
        version: WorkflowManifestVersion::V2026_11_06,
        graph: graph(
            json!({
                "version": "2026-10-03", "dependencies": {},
                "nodes": [{"id": "root.a/~", "kind": "fixture.input", "config": config}]
            }),
            &["root.a/~"],
        ),
        input_schema: sample().input_schema,
    };
    let payload = serde_json::to_value(&manifest).unwrap();
    assert_eq!(payload["version"], "2026-11-06");
    for derived in [
        "description",
        "interface",
        "workflow_id",
        "parameter_types",
        "loop_orders",
        "definition",
        "execution_order",
    ] {
        assert!(payload.get(derived).is_none());
    }
    assert_eq!(payload["graph"]["execution_order"], json!(["root.a/~"]));
    let decoded = round_trip(&manifest);
    let node = decoded.node_definition("root.a/~").unwrap();
    assert_eq!(node.kind, "fixture.input");
    assert_eq!(node.config, config);
    assert!(std::ptr::eq(node, &decoded.graph.definition.nodes[0]));
    assert!(decoded.node_definition("root.a").is_none());
    let identity =
        WorkflowId::from_definition(&decoded.graph.definition, &["root.a/~".into()]).unwrap();
    let mut expected_graph = sample().description().unwrap();
    expected_graph.workflow_id = identity.clone();
    let mut expected_interface = sample().interface().unwrap();
    expected_interface.workflow_id = identity.clone();
    assert_eq!(decoded.description().unwrap(), expected_graph);
    assert_eq!(decoded.interface().unwrap(), expected_interface);
    assert_eq!(decoded.workflow_id().unwrap(), identity);
    assert_eq!(
        mf_runtime::describe_graph(&decoded.graph).unwrap(),
        expected_graph
    );
    assert_eq!(
        decoded.description().unwrap().event_schema_version(),
        mf_telemetry::EVENT_SCHEMA_VERSION
    );
    assert!(
        serde_json::to_value(decoded.description().unwrap()).unwrap()["execution"]
            .get("event_schema_version")
            .is_none()
    );

    let mut stream = decoded.clone();
    stream.graph.definition.execution = Some(serde_json::from_value(json!({})).unwrap());
    assert_eq!(
        stream.description().unwrap().event_schema_version(),
        mf_telemetry::STREAM_EVENT_SCHEMA_VERSION
    );
    stream
        .input_schema
        .inputs
        .get_mut("root.a/~")
        .unwrap()
        .get_mut("port.a/~")
        .unwrap()
        .binding
        .default = Some(json!("fallback"));
    assert_eq!(
        stream.interface().unwrap().version,
        WorkflowInterfaceVersion::V2026_10_16
    );

    for missing in ["graph", "input_schema"] {
        let mut invalid = payload.clone();
        invalid.as_object_mut().unwrap().remove(missing);
        assert!(matches!(
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap())),
            Err(WorkflowManifestError::Json { .. })
        ));
    }
    for missing in ["definition", "execution_order", "relations"] {
        let mut invalid = payload.clone();
        invalid["graph"].as_object_mut().unwrap().remove(missing);
        assert!(matches!(
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap())),
            Err(WorkflowManifestError::Json { .. })
        ));
    }
    for version in ["2026-10-07", "2026-10-16", "2026-11-04", "2099-01-01"] {
        let mut invalid = payload.clone();
        invalid["version"] = version.into();
        assert!(
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap())).is_err()
        );
    }
    for field in ["description", "parameter_types", "loop_orders"] {
        let mut invalid = payload.clone();
        invalid[field] = json!({});
        assert!(matches!(
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap())),
            Err(WorkflowManifestError::Json { .. })
        ));
        let mut invalid = payload.clone();
        invalid["graph"][field] = json!({});
        assert!(matches!(
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap())),
            Err(WorkflowManifestError::Json { .. })
        ));
    }
    // The old payload is rejected, not silently adapted to an empty child tree.
    let legacy = json!({
        "version": "2026-11-04", "definition": payload["graph"]["definition"],
        "execution_order": ["root.a/~"], "parameter_types": {}, "loop_orders": [],
        "input_schema": payload["input_schema"]
    });
    assert!(matches!(
        WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&legacy).unwrap())),
        Err(WorkflowManifestError::Json { .. })
    ));
}

fn binding_manifest() -> WorkflowManifest {
    let mut root = graph(
        json!({
            "version": "2026-10-28", "dependencies": {},
            "nodes": [{"id": "dispatch", "kind": "fixture.dispatch", "config": {
                "selected": {"version": "2099-01-01", "nodes": []},
                "ordinary": {"version": "2026-10-28", "dependencies": {},
                    "nodes": [{"id": "shadowed", "kind": "fixture.input"}]},
                "opaque": {"definition": {"nodes": "not a graph"}}
            }}]
        }),
        &["dispatch"],
    );
    let mut selected = graph(
        json!({
            "version": "2026-10-28", "dependencies": {},
            "nodes": [{"id": "value", "kind": "fixture.dispatch", "config": {"nested": false}}]
        }),
        &["value"],
    );
    bind(
        &mut selected,
        "value",
        "nested",
        graph(
            json!({
                "version": "2026-10-03", "dependencies": {},
                "nodes": [{"id": "value", "kind": "fixture.input", "config": {"legacy": "opaque provider config"}}]
            }),
            &["value"],
        ),
    );
    bind(&mut root, "dispatch", "selected", selected);
    bind(
        &mut root,
        "dispatch",
        "fallback",
        graph(
            json!({
                "version": "2026-09-26", "dependencies": {},
                "nodes": [{"id": "value", "kind": "fixture.fallback"}]
            }),
            &["value"],
        ),
    );
    WorkflowManifest {
        version: WorkflowManifestVersion::V2026_11_06,
        graph: root,
        input_schema: WorkflowInputSchema {
            inputs: [("dispatch".to_owned(), Default::default())].into(),
            ..Default::default()
        },
    }
}

#[test]
fn actual_child_trees_are_independent_of_opaque_provider_configuration() {
    let manifest = binding_manifest();
    let decoded = round_trip(&manifest);
    let description = decoded.description().unwrap();
    assert_eq!(description.loop_bodies.len(), 3);
    let key = |owner, field| mf_telemetry::event::ScopePathEntry::binding_key(owner, field);
    for (field, kind) in [
        ("selected", "fixture.dispatch"),
        ("fallback", "fixture.fallback"),
    ] {
        let body = description.loop_body(&[key("dispatch", field)]).unwrap();
        assert_eq!(body.nodes[0].id, "value");
        assert_eq!(body.nodes[0].kind, kind);
    }
    let nested = description
        .loop_body(&[key("dispatch", "selected"), key("value", "nested")])
        .unwrap();
    assert_eq!(nested.nodes[0].kind, "fixture.input");
    assert_eq!(description.body_paths_for_node(&[], "dispatch").len(), 2);
    assert_eq!(description.static_node_count().unwrap().get(), 4);
    assert!(
        decoded.graph.definition.nodes[0]
            .config
            .get("fallback")
            .is_none()
    );

    let mut opaque = manifest.clone();
    opaque.graph.definition.nodes[0].config = json!({"nodes": 42, "selected": null});
    let opaque = round_trip(&opaque);
    assert_eq!(
        opaque.description().unwrap().loop_bodies,
        description.loop_bodies
    );
    let mut no_children = manifest.clone();
    no_children.graph.children.clear();
    assert!(
        round_trip(&no_children)
            .description()
            .unwrap()
            .loop_bodies
            .is_empty()
    );
    assert_eq!(no_children.graph.definition, manifest.graph.definition);

    // A binding not present in JSON is still a real child, even for the same
    // provider kind used elsewhere without any children.
    let mut extra = manifest.clone();
    let body = child(&mut extra.graph, "dispatch", "fallback").clone();
    bind(&mut extra.graph, "dispatch", "opaque", body);
    assert_eq!(
        round_trip(&extra).description().unwrap().loop_bodies.len(),
        4
    );
}

#[test]
fn identity_covers_actual_children_and_their_orders_without_discovering_config_graphs() {
    let mut manifest = binding_manifest();
    let fallback = child(&mut manifest.graph, "dispatch", "fallback");
    fallback
        .definition
        .nodes
        .push(serde_json::from_value(json!({"id": "other", "kind": "fixture.input"})).unwrap());
    fallback.execution_order.push("other".into());
    let identity = manifest.graph.workflow_id().unwrap();
    assert_eq!(manifest.workflow_id().unwrap(), identity);
    assert_eq!(manifest.description().unwrap().workflow_id, identity);

    for mutate_order in [false, true] {
        let mut changed = manifest.clone();
        let fallback = child(&mut changed.graph, "dispatch", "fallback");
        if mutate_order {
            fallback.execution_order.reverse();
        } else {
            fallback.definition.nodes[0].config = json!({"actual_child_change": true});
        }
        assert_eq!(changed.graph.definition, manifest.graph.definition);
        assert_eq!(
            changed.graph.execution_order,
            manifest.graph.execution_order
        );
        assert_ne!(changed.graph.workflow_id().unwrap(), identity);
        assert_eq!(
            changed.description().unwrap().workflow_id,
            changed.workflow_id().unwrap()
        );
    }

    let mut opaque = manifest.clone();
    opaque.graph.definition.nodes[0].config["ordinary"]["version"] = json!("2099-01-01");
    assert_eq!(opaque.graph.children, manifest.graph.children);
    assert_ne!(opaque.workflow_id().unwrap(), identity);
    assert_eq!(
        round_trip(&opaque).description().unwrap().loop_bodies,
        manifest.description().unwrap().loop_bodies
    );
}

#[test]
fn every_actual_child_requires_a_complete_valid_order_and_known_owner() {
    let manifest = binding_manifest();
    for (case, diagnostic) in [
        ("missing_outer_order", "does not match the node count"),
        ("missing_nested_order", "does not match the node count"),
        ("unknown_outer_node", "names unknown node `unknown`"),
        ("unknown_nested_node", "names unknown node `unknown`"),
        ("unknown_owner", "unknown owner `unknown`"),
        ("unknown_empty_owner", "unknown owner `unknown`"),
        (
            "blank_binding",
            "compiled child binding names must be nonblank",
        ),
        (
            "whitespace_binding",
            "compiled child binding names must be nonblank",
        ),
    ] {
        let mut invalid = manifest.clone();
        match case {
            "missing_outer_order" => child(&mut invalid.graph, "dispatch", "selected")
                .execution_order
                .clear(),
            "missing_nested_order" => child(
                child(&mut invalid.graph, "dispatch", "selected"),
                "value",
                "nested",
            )
            .execution_order
            .clear(),
            "unknown_outer_node" => {
                child(&mut invalid.graph, "dispatch", "selected").execution_order[0] =
                    "unknown".into()
            }
            "unknown_nested_node" => {
                child(
                    child(&mut invalid.graph, "dispatch", "selected"),
                    "value",
                    "nested",
                )
                .execution_order[0] = "unknown".into()
            }
            "unknown_owner" => {
                let children = invalid
                    .graph
                    .children
                    .remove(&DefinitionId::from("dispatch"))
                    .unwrap();
                invalid.graph.children.insert("unknown".into(), children);
            }
            "unknown_empty_owner" => {
                invalid
                    .graph
                    .children
                    .insert("unknown".into(), Default::default());
            }
            _ => {
                let fields = invalid.graph.children.get_mut(&"dispatch".into()).unwrap();
                let body = fields.remove("selected").unwrap();
                fields.insert(if case == "blank_binding" { "" } else { " " }.into(), body);
            }
        }
        for error in errors(&invalid) {
            assert!(
                matches!(&error, WorkflowManifestError::Graph { source: mf_runtime::GraphDescriptionError::Invalid { message } } if message.contains(diagnostic)),
                "{case}: {error}"
            );
            assert!(
                error
                    .source()
                    .unwrap()
                    .is::<mf_runtime::GraphDescriptionError>()
            );
            assert!(error.source().unwrap().source().is_none());
        }
    }
}

fn loop_manifest() -> WorkflowManifest {
    let mut root = graph(
        json!({
            "version": "2026-09-29", "dependencies": {},
            "nodes": [{"id": "repeat", "kind": "workflow.loop", "config": {"body": "opaque", "max_iterations": 1}}]
        }),
        &["repeat"],
    );
    let mut body = graph(
        json!({
            "version": "2026-09-29", "dependencies": {},
            "nodes": [{"id": "%loop", "kind": "%loop"},
                {"id": "inner", "kind": "workflow.loop", "config": {"body": false}}]
        }),
        &["%loop", "inner"],
    );
    body.relations.key = Some("repeat".into());
    body.relations.scope = Some(ScopeRelations {
        input: "%loop".into(),
        values: [("counter".into(), ValueType::Int64)].into(),
        state_outputs: [("counter".into(), "counter".into())].into(),
        writes: Default::default(),
        exits: Default::default(),
    });
    let mut inner = graph(
        json!({
            "version": "2026-09-29", "dependencies": {},
            "nodes": [{"id": "%loop", "kind": "%loop"}]
        }),
        &["%loop"],
    );
    inner.relations.key = Some("inner".into());
    inner.relations.scope = Some(ScopeRelations {
        input: "%loop".into(),
        values: [("counter".into(), ValueType::Int64)].into(),
        state_outputs: [("counter".into(), "counter".into())].into(),
        writes: Default::default(),
        exits: Default::default(),
    });
    bind(&mut body, "inner", "body", inner);
    bind(&mut root, "repeat", "body", body);
    WorkflowManifest {
        version: WorkflowManifestVersion::V2026_11_06,
        graph: root,
        input_schema: Default::default(),
    }
}

#[test]
fn nested_builtin_bodies_use_actual_orders_and_validate_synthetic_sources() {
    let manifest = loop_manifest();
    let description = round_trip(&manifest).description().unwrap();
    assert_eq!(description.loop_bodies.len(), 2);
    assert_eq!(
        description.loop_bodies[0]
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        ["%loop", "inner"]
    );
    assert_eq!(description.loop_bodies[1].path, ["repeat", "inner"]);
    for body in &description.loop_bodies {
        assert_eq!(body.source.as_deref(), Some("%loop"));
    }
    let mut reordered = manifest.clone();
    child(&mut reordered.graph, "repeat", "body")
        .execution_order
        .reverse();
    assert_eq!(
        reordered.description().unwrap().loop_bodies[0].nodes[0].id,
        "inner"
    );

    for case in [
        "wrong_child",
        "wrong_root",
        "duplicate_node",
        "duplicate_order",
        "missing_source",
        "unknown_source_relation",
        "incomplete_body_order",
        "incomplete_root",
        "schema",
    ] {
        let mut invalid = manifest.clone();
        match case {
            "wrong_child" => {
                child(child(&mut invalid.graph, "repeat", "body"), "inner", "body")
                    .execution_order[0] = "unknown".into()
            }
            "wrong_root" => invalid.graph.execution_order[0] = "unknown".into(),
            "duplicate_node" => {
                let node = invalid.graph.definition.nodes[0].clone();
                invalid.graph.definition.nodes.push(node);
            }
            "duplicate_order" => {
                invalid.graph.definition.nodes.push(
                    serde_json::from_value(json!({"id": "other", "kind": "fixture.node"})).unwrap(),
                );
                invalid.graph.execution_order.push("repeat".into());
            }
            "missing_source" => {
                let body = child(&mut invalid.graph, "repeat", "body");
                body.definition.nodes.remove(0);
                body.execution_order.remove(0);
            }
            "unknown_source_relation" => {
                child(&mut invalid.graph, "repeat", "body")
                    .relations
                    .scope
                    .as_mut()
                    .unwrap()
                    .input = "unknown".into();
            }
            "incomplete_body_order" => {
                child(&mut invalid.graph, "repeat", "body")
                    .execution_order
                    .remove(0);
            }
            "incomplete_root" => invalid.graph.execution_order.clear(),
            _ => invalid.input_schema = sample().input_schema,
        }
        errors(&invalid);
    }
}

#[test]
fn newer_static_children_do_not_enable_startup_inputs_on_legacy_roots() {
    for feature in ["order_edges", "clusters"] {
        let mut definition = json!({
            "version": "2026-10-23", "dependencies": {},
            "nodes": [{"id": "source", "kind": "fixture.source"}, {"id": "sink", "kind": "fixture.sink"}]
        });
        definition[if feature == "order_edges" {
            "edges"
        } else {
            feature
        }] = if feature == "order_edges" {
            json!([{"from_node": "source", "to_node": "sink"}])
        } else {
            json!([{"label": "Child work", "annotation": "Scoped child metadata.", "nodes": ["source", "sink"]}])
        };
        let mut root = graph(
            json!({
                "version": "2026-09-29", "dependencies": {},
                "nodes": [{"id": "dispatch", "kind": "user.dispatch"},
                    {"id": "independent", "kind": "fixture.input"},
                    {"id": "data-target", "kind": "fixture.sink"},
                    {"id": "control-target", "kind": "fixture.sink"}],
                "edges": [{"from_node": "dispatch", "from_output": "value", "to_node": "data-target", "to_input": "value"}],
                "control_edges": [{"from_node": "dispatch", "from_output": "ready", "to_node": "control-target"}]
            }),
            &["dispatch", "independent", "data-target", "control-target"],
        );
        bind(
            &mut root,
            "dispatch",
            "selected",
            graph(definition, &["source", "sink"]),
        );
        let manifest = WorkflowManifest {
            version: WorkflowManifestVersion::V2026_11_06,
            graph: root,
            input_schema: Default::default(),
        };
        let decoded = round_trip(&manifest);
        let description = decoded.description().unwrap();
        assert_eq!(description.loop_bodies.len(), 1);
        assert_eq!(
            description.version,
            if feature == "order_edges" {
                mf_telemetry::description::WorkflowDescriptionVersion::V2026_10_19
            } else {
                mf_telemetry::description::WorkflowDescriptionVersion::V2026_10_23
            }
        );
        assert!(description.execution.is_some());
        let body = &description.loop_bodies[0];
        if feature == "order_edges" {
            assert_eq!(body.order_edges.len(), 1);
            assert_eq!(body.order_edges[0].from_node, "source");
            assert_eq!(body.order_edges[0].to_node, "sink");
        } else {
            assert_eq!(body.clusters.len(), 1);
            assert_eq!(body.clusters[0].nodes, ["source", "sink"]);
        }
        assert_eq!(
            decoded.graph.definition.version,
            mf_runtime::WorkflowDefinitionVersion::V2026_09_29
        );
        let interface = decoded.interface().unwrap();
        assert_eq!(
            interface.schema.inputs,
            [
                ("dispatch".to_owned(), Default::default()),
                ("independent".to_owned(), Default::default())
            ]
            .into()
        );
        assert!(interface.schema.stdin.is_empty());
        assert_eq!(interface.workflow_id, decoded.workflow_id().unwrap());
        interface.validate_for_description(&description).unwrap();
        assert_eq!(decoded.input_schema, WorkflowInputSchema::default());
        decoded
            .validate_prepared_schema(&decoded.input_schema)
            .unwrap();
        assert!(matches!(
            decoded.validate_prepared_schema(&interface.schema),
            Err(WorkflowManifestError::InterfaceDrift {
                component: "initial node declaration",
                ..
            })
        ));

        // Order prerequisites and cluster membership are validated in child scope,
        // not weakened by an independently versioned legacy root.
        let mut corrupt = manifest.clone();
        if feature == "order_edges" {
            child(&mut corrupt.graph, "dispatch", "selected")
                .execution_order
                .reverse();
        } else {
            child(&mut corrupt.graph, "dispatch", "selected")
                .definition
                .clusters[0]
                .nodes
                .push("unknown".into());
        }
        errors(&corrupt);
    }
}

fn historical_iteration_manifest() -> WorkflowManifest {
    let mut root = graph(
        json!({
            "version": "2026-09-29", "dependencies": {},
            "nodes": [{"id": "map", "kind": "builtin.iteration", "config": {"body": "opaque"}}]
        }),
        &["map"],
    );
    let mut body = graph(
        json!({
            "version": "2026-10-19", "dependencies": {},
            "nodes": [{"id": "%iteration", "kind": "builtin.iteration_input"}, {"id": "dispatch", "kind": "user.dispatch"}]
        }),
        &["%iteration", "dispatch"],
    );
    body.relations.key = Some("map".into());
    body.relations.scope = Some(ScopeRelations {
        input: "%iteration".into(),
        values: [("item".into(), ValueType::String)].into(),
        state_outputs: Default::default(),
        writes: Default::default(),
        exits: Default::default(),
    });
    bind(
        &mut body,
        "dispatch",
        "selected",
        graph(
            json!({
                "version": "2026-10-19", "dependencies": {},
                "nodes": [{"id": "value", "kind": "fixture.value"}, {"id": "sink", "kind": "fixture.sink"}]
            }),
            &["value", "sink"],
        ),
    );
    bind(&mut root, "map", "body", body);
    WorkflowManifest {
        version: WorkflowManifestVersion::V2026_11_06,
        graph: root,
        input_schema: Default::default(),
    }
}

#[test]
fn historical_iteration_projection_validates_all_actual_descendants_first() {
    let manifest = historical_iteration_manifest();
    let decoded = round_trip(&manifest);
    assert!(decoded.description().unwrap().loop_bodies.is_empty());
    assert!(decoded.interface().unwrap().schema.inputs.is_empty());
    for case in [
        "incomplete_parent",
        "unknown_descendant",
        "incomplete_descendant",
        "duplicate_descendant",
        "unknown_owner",
        "reversed_prerequisite",
        "cycle",
        "missing_source",
    ] {
        let mut corrupt = manifest.clone();
        let body = child(&mut corrupt.graph, "map", "body");
        match case {
            "incomplete_parent" => body.execution_order.clear(),
            "missing_source" => {
                body.definition.nodes.remove(0);
                body.execution_order.remove(0);
            }
            "unknown_owner" => {
                body.children.insert("unknown".into(), Default::default());
            }
            _ => {
                let descendant = child(body, "dispatch", "selected");
                match case {
                    "unknown_descendant" => descendant.execution_order[0] = "unknown".into(),
                    "incomplete_descendant" => descendant.execution_order.clear(),
                    "duplicate_descendant" => descendant.execution_order[1] = "value".into(),
                    "reversed_prerequisite" => {
                        descendant
                            .definition
                            .order_edges
                            .push(mf_runtime::OrderEdgeDefinition {
                                from_node: "sink".into(),
                                to_node: "value".into(),
                            })
                    }
                    _ => {
                        descendant
                            .definition
                            .order_edges
                            .push(mf_runtime::OrderEdgeDefinition {
                                from_node: "value".into(),
                                to_node: "sink".into(),
                            });
                        descendant
                            .definition
                            .order_edges
                            .push(mf_runtime::OrderEdgeDefinition {
                                from_node: "sink".into(),
                                to_node: "value".into(),
                            });
                    }
                }
            }
        }
        for error in errors(&corrupt) {
            assert!(
                matches!(error, WorkflowManifestError::Graph { .. }),
                "{case}: {error}"
            );
        }
    }
    // Newer child metadata prevents historical omission, preserving its scope.
    let mut visible = manifest;
    child(
        child(&mut visible.graph, "map", "body"),
        "dispatch",
        "selected",
    )
    .definition
    .order_edges
    .push(mf_runtime::OrderEdgeDefinition {
        from_node: "value".into(),
        to_node: "sink".into(),
    });
    assert_eq!(
        round_trip(&visible)
            .description()
            .unwrap()
            .loop_bodies
            .len(),
        2
    );
}

#[test]
fn historical_projection_follows_explicit_stateless_frames_not_provider_names() {
    let mut manifest = historical_iteration_manifest();
    manifest.graph.definition.nodes[0].kind = "renamed.dispatch".into();
    let bindings = manifest.graph.children.get_mut(&"map".into()).unwrap();
    let mut body = bindings.remove("body").unwrap();
    body.relations.key = None;
    body.definition.nodes[0].id = "item-source".into();
    body.definition.nodes[0].kind = "renamed.input".into();
    body.execution_order[0] = "item-source".into();
    body.relations.scope.as_mut().unwrap().input = "item-source".into();
    bindings.insert("selected".into(), body);
    assert!(
        round_trip(&manifest)
            .description()
            .unwrap()
            .loop_bodies
            .is_empty()
    );

    let mut callable = manifest.clone();
    child(&mut callable.graph, "map", "selected")
        .relations
        .scope = None;
    assert_eq!(
        round_trip(&callable)
            .description()
            .unwrap()
            .loop_bodies
            .len(),
        2
    );
    let mut stateful = manifest;
    child(&mut stateful.graph, "map", "selected")
        .relations
        .scope
        .as_mut()
        .unwrap()
        .state_outputs
        .insert("item".into(), "item".into());
    assert_eq!(
        round_trip(&stateful)
            .description()
            .unwrap()
            .loop_bodies
            .len(),
        2
    );

    let mut builtin_without_frame = historical_iteration_manifest();
    child(&mut builtin_without_frame.graph, "map", "body")
        .relations
        .scope = None;
    assert_eq!(
        round_trip(&builtin_without_frame)
            .description()
            .unwrap()
            .loop_bodies
            .len(),
        2
    );
}

#[test]
fn omitted_frames_still_validate_all_relation_endpoints_and_instruction_collisions() {
    let mut manifest = historical_iteration_manifest();
    let body = child(&mut manifest.graph, "map", "body");
    body.definition.nodes.push(
        serde_json::from_value(json!({"id": "instruction", "kind": "foreign.instruction"}))
            .unwrap(),
    );
    body.execution_order.push("instruction".into());
    assert!(
        round_trip(&manifest)
            .description()
            .unwrap()
            .loop_bodies
            .is_empty()
    );
    for case in [
        "unknown_input",
        "unknown_write",
        "unknown_exit",
        "input_write_collision",
        "input_exit_collision",
        "unknown_write_variable",
        "unknown_state_variable",
        "blank_variable",
        "blank_state_output",
        "colliding_state_outputs",
        "nonidentity_state_output",
        "index_override",
        "immutable_write",
        "immutable_exit",
        "write_exit_collision",
        "source_owning_children",
        "write_owning_children",
        "exit_owning_children",
        "unknown_stream_node",
        "blank_stream_port",
        "blank_stream_name",
    ] {
        let mut corrupt = manifest.clone();
        let body = child(&mut corrupt.graph, "map", "body");
        let scope = body.relations.scope.as_mut().unwrap();
        match case {
            "unknown_input" => scope.input = "missing".into(),
            "unknown_write" => {
                scope.writes.insert("missing".into(), "item".into());
            }
            "unknown_exit" => {
                scope.exits.insert("missing".into());
            }
            "input_write_collision" => {
                scope.writes.insert("%iteration".into(), "item".into());
            }
            "input_exit_collision" => {
                scope.exits.insert("%iteration".into());
            }
            "unknown_write_variable" => {
                scope.writes.insert("dispatch".into(), "missing".into());
            }
            "unknown_state_variable" => {
                scope
                    .state_outputs
                    .insert("missing".into(), "result".into());
            }
            "blank_variable" => {
                scope.values.insert(" ".into(), ValueType::String);
            }
            "blank_state_output" => {
                scope.state_outputs.insert("item".into(), " ".into());
            }
            "colliding_state_outputs" => {
                scope.values.insert("other".into(), ValueType::String);
                scope.state_outputs.insert("item".into(), "result".into());
                scope.state_outputs.insert("other".into(), "result".into());
            }
            "nonidentity_state_output" => {
                scope.state_outputs.insert("item".into(), "result".into());
            }
            "index_override" => {
                scope.values.insert("index".into(), ValueType::Int64);
            }
            "immutable_write" => {
                scope.writes.insert("instruction".into(), "item".into());
            }
            "immutable_exit" => {
                scope.exits.insert("instruction".into());
            }
            "write_exit_collision" => {
                scope.state_outputs.insert("item".into(), "item".into());
                scope.writes.insert("instruction".into(), "item".into());
                scope.exits.insert("instruction".into());
            }
            "source_owning_children" => scope.input = "dispatch".into(),
            "write_owning_children" => {
                scope.state_outputs.insert("item".into(), "item".into());
                scope.writes.insert("dispatch".into(), "item".into());
            }
            "exit_owning_children" => {
                scope.state_outputs.insert("item".into(), "item".into());
                scope.exits.insert("dispatch".into());
            }
            _ => {
                let (name, node, port) = match case {
                    "unknown_stream_node" => ("events", "missing", "item"),
                    "blank_stream_port" => ("events", "%iteration", " "),
                    _ => (" ", "%iteration", "item"),
                };
                body.relations.streams.insert(
                    name.into(),
                    serde_json::from_value(json!({"node": node, "port": port})).unwrap(),
                );
            }
        }
        let failures = errors(&corrupt).into_iter().chain([
            corrupt.description().unwrap_err(),
            corrupt.interface().unwrap_err(),
        ]);
        for error in failures {
            assert!(
                matches!(
                    &error,
                    WorkflowManifestError::Graph {
                        source: mf_runtime::GraphDescriptionError::Invalid { .. }
                    }
                ),
                "{case}: {error}"
            );
            assert!(
                error
                    .source()
                    .unwrap()
                    .is::<mf_runtime::GraphDescriptionError>()
            );
            if case == "nonidentity_state_output" {
                assert!(
                    error
                        .to_string()
                        .contains("preserve current-frame variable names")
                );
            }
        }
    }
}

#[test]
fn explicit_relations_round_trip_without_kind_or_configuration_inference() {
    let mut manifest = binding_manifest();
    let body = child(&mut manifest.graph, "dispatch", "selected");
    for id in ["source", "other-source", "store", "leave"] {
        body.definition.nodes.push(
            serde_json::from_value(json!({"id": id, "kind": "renamed.instruction"})).unwrap(),
        );
        body.execution_order.push(id.into());
    }
    body.relations.scope = Some(ScopeRelations {
        input: "source".into(),
        values: [("item".into(), ValueType::String)].into(),
        state_outputs: [("item".into(), "item".into())].into(),
        writes: [("store".into(), "item".into())].into(),
        exits: ["leave".into()].into(),
    });
    body.relations.streams.insert(
        "events".into(),
        serde_json::from_value(json!({"node": "value", "port": "item"})).unwrap(),
    );
    let decoded = round_trip(&manifest);
    let description = decoded.description().unwrap();
    let key = mf_telemetry::event::ScopePathEntry::binding_key("dispatch", "selected");
    assert_eq!(
        description.loop_body(&[key]).unwrap().source.as_deref(),
        Some("source")
    );
    decoded
        .interface()
        .unwrap()
        .validate_for_description(&description)
        .unwrap();
    let original_id = decoded.workflow_id().unwrap();
    let mut changed = decoded;
    child(&mut changed.graph, "dispatch", "selected")
        .relations
        .scope
        .as_mut()
        .unwrap()
        .input = "other-source".into();
    let key = mf_telemetry::event::ScopePathEntry::binding_key("dispatch", "selected");
    assert_eq!(
        changed
            .description()
            .unwrap()
            .loop_body(&[key])
            .unwrap()
            .source
            .as_deref(),
        Some("other-source")
    );
    assert_eq!(changed.graph.definition, manifest.graph.definition);
    assert_ne!(round_trip(&changed).workflow_id().unwrap(), original_id);
}

#[test]
fn explicit_child_keys_cannot_collide_even_when_legacy_projection_omits_them() {
    let mut manifest = historical_iteration_manifest();
    let body = child(&mut manifest.graph, "map", "body").clone();
    bind(&mut manifest.graph, "map", "other", body);
    for error in errors(&manifest) {
        assert!(error.to_string().contains("scope keys collide"));
    }
}

#[test]
fn binding_scope_keys_cannot_be_claimed_by_another_authored_node() {
    for key in [
        mf_telemetry::event::ScopePathEntry::binding_key("other", "selected"),
        mf_telemetry::event::ScopePathEntry::binding_key("dispatch", "other"),
        "unbound".into(),
    ] {
        let mut manifest = binding_manifest();
        child(&mut manifest.graph, "dispatch", "selected")
            .relations
            .key = Some(key);
        for error in errors(&manifest) {
            assert!(matches!(error, WorkflowManifestError::Graph {
                source: mf_runtime::GraphDescriptionError::Invalid { message }
            } if message == "compiled scope keys must reference their actual receiving owner and binding"));
        }
    }
    let mut manifest = binding_manifest();
    let key = mf_telemetry::event::ScopePathEntry::binding_key("dispatch", "selected");
    manifest
        .graph
        .definition
        .nodes
        .push(serde_json::from_value(json!({"id": key, "kind": "workflow.loop"})).unwrap());
    manifest.graph.execution_order.push(key.clone().into());
    manifest.input_schema.inputs.insert(key, Default::default());
    for error in errors(&manifest) {
        assert!(error.to_string().contains("collides"));
    }
}

fn sample() -> WorkflowManifest {
    WorkflowManifest {
        version: WorkflowManifestVersion::V2026_11_06,
        graph: graph(
            json!({
                "version": "2026-10-03", "dependencies": {},
                "nodes": [{"id": "root.a/~", "kind": "fixture.input"}]
            }),
            &["root.a/~"],
        ),
        input_schema: WorkflowInputSchema {
            inputs: [(
                "root.a/~".into(),
                [(
                    "port.a/~".into(),
                    WorkflowInput {
                        value_type: mf_runtime::ValueType::String,
                        required: false,
                        binding: Default::default(),
                    },
                )]
                .into(),
            )]
            .into(),
            stdin: [(
                "root.a/~".into(),
                mf_runtime::StdinRequirement::UnlessInput("port.a/~".into()),
            )]
            .into(),
        },
    }
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut bytes = MANIFEST_MAGIC.to_vec();
    bytes.extend_from_slice(&MANIFEST_FRAMING_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

#[test]
fn preserves_protocols_names_and_conditional_resources() {
    let manifest = sample();
    let mut bytes = manifest.to_bytes().unwrap();
    assert_eq!(&bytes[..8], b"MFMANIF\0");
    assert_eq!(&bytes[8..12], &1u32.to_le_bytes());
    assert_eq!(
        u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
        (bytes.len() - MANIFEST_HEADER_BYTES) as u64
    );
    bytes.extend(vec![0; MAX_MANIFEST_PADDING_BYTES]);
    assert_eq!(WorkflowManifest::from_bytes(&bytes).unwrap(), manifest);
    bytes.push(0);
    assert!(matches!(
        WorkflowManifest::from_bytes(&bytes),
        Err(WorkflowManifestError::Framing { .. })
    ));
}

#[test]
fn rejects_corrupt_framing_and_lengths_without_allocating_declared_payloads() {
    let bytes = sample().to_bytes().unwrap();
    for end in [0, 7, 19, bytes.len() - 1] {
        assert!(matches!(
            WorkflowManifest::from_bytes(&bytes[..end]),
            Err(WorkflowManifestError::Framing { .. })
        ));
    }
    let mut invalid = bytes.clone();
    invalid[0] = 0;
    assert!(matches!(
        WorkflowManifest::from_bytes(&invalid),
        Err(WorkflowManifestError::Framing { .. })
    ));
    invalid = bytes.clone();
    invalid[8..12].copy_from_slice(&2u32.to_le_bytes());
    assert!(matches!(
        WorkflowManifest::from_bytes(&invalid),
        Err(WorkflowManifestError::UnsupportedFraming { version: 2 })
    ));
    for length in [MAX_MANIFEST_PAYLOAD_BYTES as u64 + 1, u64::MAX] {
        invalid = bytes.clone();
        invalid[12..20].copy_from_slice(&length.to_le_bytes());
        assert!(matches!(
            WorkflowManifest::from_bytes(&invalid),
            Err(WorkflowManifestError::TooLarge { .. })
        ));
    }
    invalid = bytes;
    invalid.push(1);
    assert!(matches!(
        WorkflowManifest::from_bytes(&invalid),
        Err(WorkflowManifestError::Framing { .. })
    ));
}

#[test]
fn rejects_invalid_versions_and_duplicate_members_with_json_sources() {
    let payload = serde_json::to_string(&sample()).unwrap();
    for invalid in [
        payload.replace("2026-11-06", "2099-01-01"),
        payload.replacen("{", "{\"version\":\"2026-11-06\",", 1),
        payload.replace("\"required\":false", "\"required\":false,\"required\":true"),
        format!("{payload}{{}}"),
        "{".into(),
    ] {
        let error = WorkflowManifest::from_bytes(&frame(invalid.as_bytes())).unwrap_err();
        assert!(matches!(error, WorkflowManifestError::Json { .. }));
        assert!(error.source().unwrap().is::<serde_json::Error>());
    }
    // In-memory definition serialization failures preserve the native source too.
    let mut invalid = sample();
    invalid.graph.definition.version = mf_runtime::WorkflowDefinitionVersion::V2026_09_29;
    invalid
        .graph
        .definition
        .clusters
        .push(mf_runtime::ClusterDefinition {
            label: "Work".into(),
            annotation: "Metadata requiring a newer definition.".into(),
            nodes: vec!["root.a/~".into()],
        });
    // Identity serialization is a genuine telemetry boundary, unlike the
    // runtime's own authored-definition validation during graph description.
    let identity_error = invalid.graph.workflow_id().unwrap_err();
    assert!(matches!(
        &identity_error,
        mf_runtime::GraphDescriptionError::Observation {
            source: mf_telemetry::ContractError::Json { .. }
        }
    ));
    assert!(
        identity_error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<serde_json::Error>()
    );
    let error = invalid.description().unwrap_err();
    assert!(matches!(
        &error,
        WorkflowManifestError::Graph {
            source: mf_runtime::GraphDescriptionError::Definition { .. }
        }
    ));
    assert!(
        error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<serde_json::Error>()
    );
}

#[test]
fn limits_the_combined_serialized_payload() {
    let mut manifest = sample();
    manifest.graph.definition.nodes[0].config =
        json!({"opaque": "x".repeat(MAX_MANIFEST_PAYLOAD_BYTES)});
    assert!(matches!(
        manifest.to_bytes(),
        Err(WorkflowManifestError::TooLarge { .. })
    ));
}

#[test]
fn reports_exact_interface_drift_before_accepting_prepared_schemas() {
    let manifest = sample();
    let schema = manifest.input_schema.clone();
    manifest.validate_prepared_schema(&schema).unwrap();
    for component in [
        "node",
        "port",
        "type",
        "required",
        "env",
        "default",
        "secret",
        "stdin",
        "condition",
    ] {
        let mut actual = schema.clone();
        match component {
            "node" => {
                actual.inputs.insert("extra".into(), Default::default());
            }
            "port" => actual.inputs.get_mut("root.a/~").unwrap().clear(),
            "stdin" => actual.stdin.clear(),
            "type" | "required" | "env" | "default" | "secret" => {
                let input = actual
                    .inputs
                    .get_mut("root.a/~")
                    .unwrap()
                    .get_mut("port.a/~")
                    .unwrap();
                match component {
                    "type" => input.value_type = mf_runtime::ValueType::Int64,
                    "required" => input.required = true,
                    "env" => input.binding.env = Some("CONFIG_VALUE".into()),
                    "default" => input.binding.default = Some(json!("default")),
                    _ => input.binding.secret = true,
                }
            }
            _ => {
                actual
                    .stdin
                    .insert("root.a/~".into(), mf_runtime::StdinRequirement::Always);
            }
        }
        let error = manifest.validate_prepared_schema(&actual).unwrap_err();
        assert!(matches!(
            error,
            WorkflowManifestError::InterfaceDrift { .. }
        ));
        let diagnostic = error.to_string();
        assert!(
            diagnostic.contains(if component == "node" {
                "/extra"
            } else {
                "/root.a~1~0"
            }),
            "{diagnostic}"
        );
        if matches!(
            component,
            "port" | "type" | "required" | "env" | "default" | "secret"
        ) {
            assert!(diagnostic.contains("/port.a~1~0"), "{diagnostic}");
        }
    }
}
