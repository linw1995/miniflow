use mf_runtime::{
    MANIFEST_FRAMING_VERSION, MANIFEST_HEADER_BYTES, MANIFEST_MAGIC, MAX_MANIFEST_PADDING_BYTES,
    MAX_MANIFEST_PAYLOAD_BYTES, WorkflowInput, WorkflowInputSchema, WorkflowInterfaceVersion,
    WorkflowManifest, WorkflowManifestError, WorkflowManifestVersion,
};
use mf_telemetry::identity::WorkflowId;
use std::error::Error;

fn declarations(value: serde_json::Value) -> mf_runtime::NodeParameterTypes {
    serde_json::from_value(value).unwrap()
}

#[test]
fn compiled_manifests_derive_views_from_original_facts() {
    let config = serde_json::json!({
        "model": {"$ref": "parameters.model"}, "temperature": 0.25,
        "options": {"labels": ["first", "second"]}
    });
    let definition = serde_json::from_value(serde_json::json!({
        "version": "2026-10-03", "dependencies": {},
        "nodes": [{"id": "root.a/~", "kind": "fixture.input", "config": config}]
    }))
    .unwrap();
    let manifest = WorkflowManifest {
        version: WorkflowManifestVersion::V2026_11_04,
        definition,
        parameter_types: declarations(serde_json::json!({"fixture.input": {
            "model": "value", "temperature": "value", "options": "value"
        }})),
        execution_order: vec!["root.a/~".into()],
        loop_orders: vec![],
        input_schema: sample().input_schema,
    };
    let payload = serde_json::to_value(&manifest).unwrap();
    assert_eq!(payload["version"], "2026-11-04");
    for derived in ["description", "interface", "workflow_id"] {
        assert!(payload.get(derived).is_none());
    }
    let decoded = WorkflowManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap();
    assert_eq!(decoded, manifest);
    let node = decoded.node_definition("root.a/~").unwrap();
    assert_eq!(node.kind, "fixture.input");
    assert_eq!(node.config, config);
    assert!(std::ptr::eq(node, &decoded.definition.nodes[0]));
    assert!(decoded.node_definition("root.a").is_none());
    let identity = WorkflowId::from_definition(&decoded.definition, &["root.a/~".into()]).unwrap();
    let mut expected_graph = sample().description().unwrap();
    expected_graph.workflow_id = identity.clone();
    let mut expected_interface = sample().interface().unwrap();
    expected_interface.workflow_id = identity.clone();
    assert_eq!(decoded.description().unwrap(), expected_graph);
    assert_eq!(decoded.interface().unwrap(), expected_interface);
    assert_eq!(decoded.workflow_id().unwrap(), identity);
    let description = decoded.description().unwrap();
    assert_eq!(
        description.event_schema_version(),
        mf_telemetry::EVENT_SCHEMA_VERSION
    );
    assert!(
        serde_json::to_value(&description).unwrap()["execution"]
            .get("event_schema_version")
            .is_none()
    );
    let mut stream = decoded.clone();
    let definition = &mut stream.definition;
    definition.execution = Some(serde_json::from_value(serde_json::json!({})).unwrap());
    assert_eq!(
        stream.description().unwrap().event_schema_version(),
        mf_telemetry::STREAM_EVENT_SCHEMA_VERSION
    );
    let input_schema = &mut stream.input_schema;
    input_schema
        .inputs
        .get_mut("root.a/~")
        .unwrap()
        .get_mut("port.a/~")
        .unwrap()
        .binding
        .default = Some(serde_json::json!("fallback"));
    assert_eq!(
        stream.interface().unwrap().version,
        WorkflowInterfaceVersion::V2026_10_16
    );

    for missing in [
        "definition",
        "parameter_types",
        "execution_order",
        "input_schema",
    ] {
        let mut invalid = payload.clone();
        invalid.as_object_mut().unwrap().remove(missing);
        assert!(matches!(
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap())),
            Err(WorkflowManifestError::Json { .. })
        ));
    }
    for version in ["2026-10-07", "2026-10-16", "2099-01-01"] {
        let mut invalid = payload.clone();
        invalid["version"] = version.into();
        assert!(
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap())).is_err()
        );
    }
    let mut invalid = payload;
    invalid["description"] = serde_json::to_value(expected_graph).unwrap();
    assert!(WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap())).is_err());
}

#[test]
fn nested_binding_views_require_consistent_orders_for_each_frozen_field() {
    let definition = serde_json::from_value(serde_json::json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [{"id": "dispatch", "kind": "fixture.dispatch", "config": {
            "selected": {
                "version": "2026-10-28", "dependencies": {},
                "nodes": [{"id": "inner", "kind": "fixture.dispatch", "config": {
                    "nested": {
                        "version": "2026-10-28", "dependencies": {},
                        "nodes": [{"id": "value", "kind": "fixture.input",
                            "config": {"legacy": {"$flow": "opaque provider config"}}}]
                    }
                }}]
            },
            "opaque": {"nested": {"$flow": "not an immediate marker"}},
            "ordinary": {"definition": {
                "version": "2026-10-28", "dependencies": {},
                "nodes": [{"id": "shadowed", "kind": "fixture.input"}]
            }}
        }}]
    }))
    .unwrap();
    let outer_key = mf_telemetry::event::ScopePathEntry::binding_key("dispatch", "selected");
    let inner_key = mf_telemetry::event::ScopePathEntry::binding_key("inner", "nested");
    let manifest = WorkflowManifest {
        version: WorkflowManifestVersion::V2026_11_04,
        definition,
        parameter_types: declarations(serde_json::json!({
            "fixture.dispatch": {
                "selected": {"optional": "flow"}, "nested": {"optional": "flow"},
                "opaque": "value", "ordinary": "value"
            },
            "fixture.input": {"legacy": "value"}
        })),
        execution_order: vec!["dispatch".into()],
        loop_orders: vec![
            mf_runtime::LoopExecutionOrder {
                path: vec![outer_key.clone()],
                execution_order: vec!["inner".into()],
            },
            mf_runtime::LoopExecutionOrder {
                path: vec![outer_key.clone(), inner_key.clone()],
                execution_order: vec!["value".into()],
            },
        ],
        input_schema: WorkflowInputSchema {
            inputs: [("dispatch".to_owned(), Default::default())].into(),
            ..Default::default()
        },
    };
    let decoded = WorkflowManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap();
    assert_eq!(decoded, manifest);
    let graph = decoded.description().unwrap();
    assert_eq!(graph.loop_bodies.len(), 2);
    assert_eq!(graph.loop_bodies[0].path, vec![outer_key]);
    assert_eq!(graph.loop_bodies[1].path, manifest.loop_orders[1].path);
    assert_eq!(graph.loop_bodies[1].nodes[0].id, "value");

    // Orders are execution prerequisites, not field authority: deleting any or
    // all of them cannot make a declared executable child ordinary provider JSON.
    let mut ordinary = manifest.clone();
    ordinary.loop_orders.clear();
    assert!(ordinary.description().is_err());
    ordinary.parameter_types = declarations(serde_json::json!({"fixture.dispatch": {
        "selected": "value", "nested": "value", "opaque": "value", "ordinary": "value"
    }}));
    assert!(ordinary.description().unwrap().loop_bodies.is_empty());
    assert!(ordinary.to_bytes().is_ok());

    for (case, diagnostic) in [
        ("missing_outer", "missing compiled Flow parameter order"),
        ("missing_nested", "missing compiled Flow parameter order"),
        ("missing_all", "missing compiled Flow parameter order"),
        ("missing_kind", "missing parameter types"),
        ("missing_plain_kind", "missing parameter types"),
        (
            "unknown_outer_field",
            "missing compiled Flow parameter order",
        ),
        ("incomplete_outer", "does not match the node count"),
        ("unknown_outer_node", "names unknown node `unknown`"),
        ("wrong_frozen_field", "names unknown node `shadowed`"),
    ] {
        let mut invalid = manifest.clone();
        match case {
            "missing_outer" => {
                invalid.loop_orders.remove(0);
            }
            "missing_nested" => {
                invalid.loop_orders.pop();
            }
            "missing_all" => invalid.loop_orders.clear(),
            "missing_kind" => {
                invalid.parameter_types.remove("fixture.dispatch");
            }
            "missing_plain_kind" => {
                invalid.parameter_types.remove("fixture.input");
            }
            "unknown_outer_field" => {
                invalid.loop_orders.pop();
                invalid.loop_orders[0].path[0] =
                    mf_telemetry::event::ScopePathEntry::binding_key("dispatch", "unknown");
            }
            "incomplete_outer" => invalid.loop_orders[0].execution_order.clear(),
            "unknown_outer_node" => {
                invalid.loop_orders[0].execution_order[0] = "unknown".into();
            }
            _ => {
                invalid.loop_orders[1].execution_order[0] = "shadowed".into();
            }
        }
        for error in [
            invalid.to_bytes().unwrap_err(),
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap()))
                .unwrap_err(),
        ] {
            assert!(
                matches!(error, WorkflowManifestError::Graph { .. }),
                "{case}"
            );
            let message = error.to_string();
            assert!(message.contains(diagnostic), "{case}: {message}");
        }
    }
}

#[test]
fn nested_loop_views_use_frozen_orders_and_reject_inconsistent_facts() {
    let definition = serde_json::from_value(serde_json::json!({
        "version": "2026-09-29", "dependencies": {},
        "nodes": [{"id": "repeat", "kind": "workflow.loop", "config": {
            "max_iterations": 1, "variables": [], "body": {
                "version": "2026-09-29", "dependencies": {},
                "nodes": [
                    {"id": "%loop", "kind": "%loop", "config": {}},
                    {"id": "inner", "kind": "workflow.loop", "config": {
                        "max_iterations": 1, "variables": [], "body": {
                            "version": "2026-09-29", "dependencies": {},
                            "nodes": [{"id": "%loop", "kind": "%loop", "config": {}}],
                            "edges": [], "outputs": []
                        }
                    }}
                ], "edges": [], "outputs": []
            }
        }}]
    }))
    .unwrap();
    let manifest = WorkflowManifest {
        version: WorkflowManifestVersion::V2026_11_04,
        definition,
        parameter_types: declarations(serde_json::json!({
            "workflow.loop": {"body": "flow", "max_iterations": "value", "variables": "value"},
            "%loop": {}
        })),
        execution_order: vec!["repeat".into()],
        loop_orders: vec![
            mf_runtime::LoopExecutionOrder {
                path: vec!["repeat".into()],
                execution_order: vec!["%loop".into(), "inner".into()],
            },
            mf_runtime::LoopExecutionOrder {
                path: vec!["repeat".into(), "inner".into()],
                execution_order: vec!["%loop".into()],
            },
        ],
        input_schema: WorkflowInputSchema::default(),
    };
    let decoded = WorkflowManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap();
    assert_eq!(decoded, manifest);
    let graph = decoded.description().unwrap();
    assert_eq!(graph.loop_bodies.len(), 2);
    assert_eq!(
        graph.loop_bodies[0]
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        ["%loop", "inner"]
    );
    assert_eq!(graph.loop_bodies[1].path, ["repeat", "inner"]);
    let mut reordered = manifest.clone();
    let loop_orders = &mut reordered.loop_orders;
    loop_orders[0].execution_order.reverse();
    assert_eq!(
        reordered.description().unwrap().loop_bodies[0]
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        ["inner", "%loop"]
    );
    for case in [
        "missing",
        "duplicate",
        "unknown",
        "wrong_node",
        "wrong_root",
        "duplicate_node",
        "duplicate_order",
        "missing_source",
        "incomplete_body_order",
        "incomplete",
        "schema",
    ] {
        let mut invalid = manifest.clone();
        let WorkflowManifest {
            definition,
            execution_order,
            loop_orders,
            input_schema,
            ..
        } = &mut invalid;
        match case {
            "missing" => {
                loop_orders.remove(0);
            }
            "duplicate" => loop_orders.push(loop_orders[0].clone()),
            "unknown" => loop_orders.push(mf_runtime::LoopExecutionOrder {
                path: vec!["unknown".into()],
                execution_order: vec!["%loop".into()],
            }),
            "wrong_node" => loop_orders[1].execution_order[0] = "unknown".into(),
            "wrong_root" => execution_order[0] = "unknown".into(),
            "duplicate_node" => {
                definition.nodes.push(definition.nodes[0].clone());
            }
            "duplicate_order" => {
                definition.nodes.push(
                    serde_json::from_value(
                        serde_json::json!({"id": "other", "kind": "fixture.node"}),
                    )
                    .unwrap(),
                );
                execution_order.push("repeat".into());
            }
            "missing_source" => {
                // Offline consumers must not recreate compiler-owned scope nodes.
                definition.nodes[0].config["body"]["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .remove(0);
            }
            "incomplete_body_order" => {
                loop_orders[0].execution_order.remove(0);
            }
            "incomplete" => execution_order.clear(),
            "schema" => *input_schema = sample().input_schema,
            _ => unreachable!(),
        }
        assert!(invalid.to_bytes().is_err(), "{case}");
        assert!(
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap())).is_err(),
            "{case}"
        );
    }
}

#[test]
fn newer_static_children_do_not_enable_startup_inputs_on_legacy_roots() {
    for feature in ["order_edges", "clusters"] {
        let mut child = serde_json::json!({
            "version": "2026-10-23", "dependencies": {},
            "nodes": [
                {"id": "source", "kind": "fixture.source"},
                {"id": "sink", "kind": "fixture.sink"}
            ]
        });
        child[if feature == "order_edges" {
            "edges"
        } else {
            feature
        }] = if feature == "order_edges" {
            serde_json::json!([{"from_node": "source", "to_node": "sink"}])
        } else {
            serde_json::json!([{
                "label": "Child work", "annotation": "Scoped child metadata.",
                "nodes": ["source", "sink"]
            }])
        };
        let manifest = WorkflowManifest {
            version: WorkflowManifestVersion::V2026_11_04,
            definition: serde_json::from_value(serde_json::json!({
                "version": "2026-09-29", "dependencies": {},
                "nodes": [
                    {"id": "dispatch", "kind": "user.dispatch", "config": {"selected": child}},
                    {"id": "independent", "kind": "fixture.input"},
                    {"id": "data-target", "kind": "fixture.sink"},
                    {"id": "control-target", "kind": "fixture.sink"}
                ],
                "edges": [{"from_node": "dispatch", "from_output": "value",
                    "to_node": "data-target", "to_input": "value"}],
                "control_edges": [{"from_node": "dispatch", "from_output": "ready",
                    "to_node": "control-target"}]
            }))
            .unwrap(),
            parameter_types: declarations(serde_json::json!({
                "user.dispatch": {"selected": "flow"},
                "fixture.input": {}, "fixture.source": {}, "fixture.sink": {}
            })),
            execution_order: vec![
                "dispatch".into(),
                "independent".into(),
                "data-target".into(),
                "control-target".into(),
            ],
            loop_orders: vec![mf_runtime::LoopExecutionOrder {
                path: vec![mf_telemetry::event::ScopePathEntry::binding_key(
                    "dispatch", "selected",
                )],
                execution_order: vec!["source".into(), "sink".into()],
            }],
            input_schema: WorkflowInputSchema::default(),
        };
        let decoded = WorkflowManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap();
        assert_eq!(decoded, manifest);
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
            decoded.definition.version,
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
        // Observation declarations must not rewrite the actual prepared schema.
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
    }
}

#[test]
fn sibling_frozen_fields_keep_identical_node_ids_in_separate_scopes() {
    let key = |field| mf_telemetry::event::ScopePathEntry::binding_key("dispatch", field);
    let manifest = WorkflowManifest {
        version: WorkflowManifestVersion::V2026_11_04,
        definition: serde_json::from_value(serde_json::json!({
            "version": "2026-10-28", "dependencies": {},
            "nodes": [{"id": "dispatch", "kind": "user.dispatch", "config": {
                "selected": {"version": "2026-10-28", "dependencies": {}, "nodes": [
                    {"id": "value", "kind": "fixture.selected"}
                ]},
                "fallback": {"version": "2026-10-28", "dependencies": {}, "nodes": [
                    {"id": "value", "kind": "fixture.fallback"}
                ]},
                "opaque": {"version": "2099-01-01", "nodes": []}
            }}]
        }))
        .unwrap(),
        parameter_types: declarations(serde_json::json!({
            "user.dispatch": {
                "selected": "flow", "fallback": "flow",
                "optional": {"optional": "flow"}, "opaque": "value"
            },
            "fixture.selected": {}, "fixture.fallback": {}
        })),
        execution_order: vec!["dispatch".into()],
        loop_orders: vec![
            mf_runtime::LoopExecutionOrder {
                path: vec![key("selected")],
                execution_order: vec!["value".into()],
            },
            mf_runtime::LoopExecutionOrder {
                path: vec![key("fallback")],
                execution_order: vec!["value".into()],
            },
        ],
        input_schema: WorkflowInputSchema {
            inputs: [("dispatch".to_owned(), Default::default())].into(),
            ..Default::default()
        },
    };
    let decoded = WorkflowManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap();
    let graph = decoded.description().unwrap();
    for field in ["selected", "fallback"] {
        let body = graph.loop_body(&[key(field)]).unwrap();
        assert_eq!(body.nodes[0].id, "value");
        assert_eq!(body.nodes[0].kind, format!("fixture.{field}"));
    }
    assert_eq!(graph.body_paths_for_node(&[], "dispatch").len(), 2);
    assert_eq!(graph.static_node_count().unwrap().get(), 3);
    let mut reordered = manifest.clone();
    reordered.loop_orders.reverse();
    assert_eq!(reordered.description().unwrap(), graph);

    assert_eq!(decoded.parameter_types, manifest.parameter_types);
    assert_eq!(
        decoded.parameter_types["user.dispatch"]["optional"],
        mf_runtime::ConfigParameterType::Optional(Box::new(mf_runtime::ConfigParameterType::Flow))
    );
    assert!(decoded.definition.nodes[0].config.get("optional").is_none());

    // A forged order cannot authorize parsing opaque, malformed provider JSON.
    // Corrupting a declared field instead preserves the native JSON source chain.
    for case in ["forged_opaque_order", "corrupt_declared_field"] {
        let mut corrupt = manifest.clone();
        if case == "forged_opaque_order" {
            corrupt.loop_orders.push(mf_runtime::LoopExecutionOrder {
                path: vec![key("opaque")],
                execution_order: vec![],
            });
        } else {
            corrupt.definition.nodes[0].config["selected"] =
                serde_json::json!({"version": "2099-01-01", "nodes": []});
        }
        for error in [
            corrupt.to_bytes().unwrap_err(),
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&corrupt).unwrap()))
                .unwrap_err(),
        ] {
            assert!(error.source().unwrap().is::<mf_telemetry::ContractError>());
            if case == "forged_opaque_order" {
                assert!(matches!(&error, WorkflowManifestError::Graph {
                    source: mf_telemetry::ContractError::Invalid { message }
                } if message.contains("unknown Loop body")));
                assert!(error.source().unwrap().source().is_none());
            } else {
                assert!(matches!(&error, WorkflowManifestError::Graph {
                    source: mf_telemetry::ContractError::Json { source }
                } if source.is_data()));
                assert!(
                    error
                        .source()
                        .unwrap()
                        .source()
                        .unwrap()
                        .is::<serde_json::Error>()
                );
            }
        }
    }
    assert_eq!(manifest.description().unwrap(), graph);
}

#[test]
fn historical_iteration_projection_still_validates_explicit_frozen_descendants() {
    let selected = mf_telemetry::event::ScopePathEntry::binding_key("dispatch", "selected");
    let manifest = WorkflowManifest {
        version: WorkflowManifestVersion::V2026_11_04,
        definition: serde_json::from_value(serde_json::json!({
            "version": "2026-09-29", "dependencies": {},
            "nodes": [{"id": "map", "kind": "builtin.iteration", "config": {"body": {
                "version": "2026-10-19", "dependencies": {}, "nodes": [
                    {"id": "%iteration", "kind": "builtin.iteration_input"},
                    {"id": "dispatch", "kind": "user.dispatch", "config": {"selected": {
                        "version": "2026-10-19", "dependencies": {},
                        "nodes": [{"id": "value", "kind": "fixture.value"}]
                    }}}
                ]
            }}}]
        }))
        .unwrap(),
        parameter_types: declarations(serde_json::json!({
            "builtin.iteration": {"body": "flow"}, "builtin.iteration_input": {},
            "user.dispatch": {"selected": "flow"}, "fixture.value": {}
        })),
        execution_order: vec!["map".into()],
        loop_orders: vec![
            mf_runtime::LoopExecutionOrder {
                path: vec!["map".into()],
                execution_order: vec!["%iteration".into(), "dispatch".into()],
            },
            mf_runtime::LoopExecutionOrder {
                path: vec!["map".into(), selected],
                execution_order: vec!["value".into()],
            },
        ],
        input_schema: WorkflowInputSchema::default(),
    };
    let decoded = WorkflowManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap();
    assert_eq!(decoded, manifest);
    assert!(decoded.description().unwrap().loop_bodies.is_empty());
    assert!(decoded.interface().unwrap().schema.inputs.is_empty());
    for (case, diagnostic) in [
        ("missing_parent", "missing compiled body execution order"),
        ("missing_all", "missing compiled body execution order"),
        (
            "missing_descendant",
            "missing compiled Flow parameter order",
        ),
        (
            "unknown_descendant",
            "compiled execution order names unknown node `unknown`",
        ),
        (
            "incomplete_descendant",
            "compiled execution order does not match the node count",
        ),
    ] {
        let mut corrupt = manifest.clone();
        match case {
            "missing_parent" => {
                corrupt.loop_orders.remove(0);
            }
            "missing_all" => corrupt.loop_orders.clear(),
            "missing_descendant" => {
                corrupt.loop_orders.pop();
            }
            "unknown_descendant" => corrupt.loop_orders[1].execution_order[0] = "unknown".into(),
            _ => corrupt.loop_orders[1].execution_order.clear(),
        }
        for error in [
            corrupt.to_bytes().unwrap_err(),
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&corrupt).unwrap()))
                .unwrap_err(),
        ] {
            assert!(
                matches!(&error, WorkflowManifestError::Graph {
                source: mf_telemetry::ContractError::Invalid { message }
            } if message.contains(diagnostic)),
                "{case}: {error}"
            );
        }
    }
}

fn sample() -> WorkflowManifest {
    WorkflowManifest {
        version: WorkflowManifestVersion::V2026_11_04,
        definition: serde_json::from_value(serde_json::json!({
            "version": "2026-10-03", "dependencies": {},
            "nodes": [{"id": "root.a/~", "kind": "fixture.input"}]
        }))
        .unwrap(),
        parameter_types: declarations(serde_json::json!({"fixture.input": {}})),
        execution_order: vec!["root.a/~".into()],
        loop_orders: vec![],
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
        payload.replace("2026-11-04", "2099-01-01"),
        payload.replacen("{", "{\"version\":\"2026-11-04\",", 1),
        payload.replace("\"required\":false", "\"required\":false,\"required\":true"),
        format!("{payload}{{}}"),
        "{".into(),
    ] {
        let error = WorkflowManifest::from_bytes(&frame(invalid.as_bytes())).unwrap_err();
        assert!(matches!(error, WorkflowManifestError::Json { .. }));
        assert!(error.source().unwrap().is::<serde_json::Error>());
    }
}

#[test]
fn limits_the_combined_serialized_payload() {
    let mut manifest = sample();
    manifest.definition.nodes[0].config =
        serde_json::json!({"opaque": "x".repeat(MAX_MANIFEST_PAYLOAD_BYTES)});
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
            "port" => {
                actual.inputs.get_mut("root.a/~").unwrap().clear();
            }
            "type" => {
                actual
                    .inputs
                    .get_mut("root.a/~")
                    .unwrap()
                    .get_mut("port.a/~")
                    .unwrap()
                    .value_type = mf_runtime::ValueType::Int64;
            }
            "required" => {
                actual
                    .inputs
                    .get_mut("root.a/~")
                    .unwrap()
                    .get_mut("port.a/~")
                    .unwrap()
                    .required = true;
            }
            "stdin" => {
                actual.stdin.clear();
            }
            "env" | "default" | "secret" => {
                let binding = &mut actual
                    .inputs
                    .get_mut("root.a/~")
                    .unwrap()
                    .get_mut("port.a/~")
                    .unwrap()
                    .binding;
                match component {
                    "env" => binding.env = Some("CONFIG_VALUE".into()),
                    "default" => binding.default = Some(serde_json::json!("default")),
                    _ => binding.secret = true,
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
