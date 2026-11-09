use mf_telemetry::{
    Count, FLOW_EVENT_SCHEMA_VERSION, FLOW_STREAM_EVENT_SCHEMA_VERSION,
    description::{WorkflowDescription, WorkflowDescriptionVersion},
    event::{Event, FlowOrigin, LifecycleEvent, NodeIdentity, ScopePathEntry, SkipCause},
    identity::RunId,
    iteration::{IterationContext, IterationDetailContext},
    observation::Observer,
    stream::{StreamIdentity, StreamPayload, StreamRecord, StreamTrigger},
    wire::WireRecord,
};
use serde_json::json;

fn count(value: i64) -> Count {
    Count::try_from(value).unwrap()
}

fn graph() -> WorkflowDescription {
    WorkflowDescription::from_json(&serde_json::to_vec(&json!({
        "version":"2026-11-09", "workflow_id":format!("sha256:{}", "b".repeat(64)),
        "execution":{"mode":"single"},
        "nodes":[{"id":"producer", "kind":"builtin.constant"}, {"id":"A", "kind":"workflow.call"}],
        "execution_order":["producer", "A"], "data_edges":[], "control_edges":[],
        "loop_bodies":[
            {"path":["producer"], "nodes":[{"id":"same", "kind":"fixture.target"}, {"id":"nested", "kind":"workflow.call"}, {"id":"repeat", "kind":"workflow.loop"}, {"id":"each", "kind":"workflow.iteration"}],
             "execution_order":["same", "nested", "repeat", "each"], "data_edges":[], "control_edges":[]},
            {"path":["producer", "nested"], "nodes":[{"id":"same", "kind":"fixture.leaf"}], "execution_order":["same"], "data_edges":[], "control_edges":[]},
            {"path":["producer", "repeat"], "source":"%loop", "state_outputs":{"value":"value"}, "nodes":[{"id":"%loop", "kind":"%loop"}, {"id":"same", "kind":"fixture.loop-leaf"}], "execution_order":["%loop", "same"], "data_edges":[], "control_edges":[]},
            {"path":["producer", "each"], "source":"%iteration", "nodes":[{"id":"%iteration", "kind":"%iteration"}, {"id":"invoke", "kind":"workflow.call"}], "execution_order":["%iteration", "invoke"], "data_edges":[], "control_edges":[]},
            {"path":["A"], "nodes":[{"id":"same", "kind":"fixture.caller"}, {"id":"invoke", "kind":"workflow.call"}], "execution_order":["same", "invoke"], "data_edges":[], "control_edges":[]}
        ]
    })).unwrap()).unwrap()
}

fn origin(graph: &WorkflowDescription, path: &[&str]) -> FlowOrigin {
    FlowOrigin {
        workflow_id: graph.workflow_id.clone(),
        path: path.iter().map(|key| (*key).into()).collect(),
    }
}

fn invocation(graph: &WorkflowDescription) -> Vec<ScopePathEntry> {
    vec![
        ScopePathEntry::Call {
            call_id: "A".into(),
        },
        ScopePathEntry::Flow {
            call_id: "invoke".into(),
            definition: origin(graph, &["producer"]),
        },
    ]
}

fn lifecycle(
    graph: &WorkflowDescription,
    path: Vec<ScopePathEntry>,
    id: &str,
    kind: &str,
    position: i64,
) -> LifecycleEvent {
    LifecycleEvent {
        workflow_id: graph.workflow_id.clone(),
        run_id: RunId::new(),
        sequence: count(2),
        event: Event::NodeStarted {
            node: NodeIdentity {
                id: id.into(),
                kind: kind.into(),
                path,
            },
            selected_inputs: vec![],
            position: count(position),
            elapsed_ns: count(1),
        },
    }
}

#[test]
fn flow_frames_jump_definitions_without_reparenting_runtime_ancestry() {
    let graph = graph();
    let path = invocation(&graph);
    assert_eq!(path[1].node_id(), "invoke");
    assert!(path[1].is_call());
    assert_eq!(path[1].index(), Count::ZERO);
    assert_eq!(graph.scope_definition_path(&path).unwrap(), ["producer"]);
    lifecycle(&graph, path.clone(), "same", "fixture.target", 0)
        .validate_for(&graph)
        .unwrap();
    assert!(
        lifecycle(&graph, path.clone(), "same", "fixture.caller", 0)
            .validate_for(&graph)
            .is_err()
    );
    let mut nested = path.clone();
    nested.push(ScopePathEntry::Call {
        call_id: "nested".into(),
    });
    assert_eq!(
        graph.scope_definition_path(&nested).unwrap(),
        ["producer", "nested"]
    );
    lifecycle(&graph, nested, "same", "fixture.leaf", 0)
        .validate_for(&graph)
        .unwrap();
    let mut pass = path;
    pass.push(ScopePathEntry::Loop {
        loop_id: "repeat".into(),
        index: count(7),
    });
    assert_eq!(
        graph.scope_definition_path(&pass).unwrap(),
        ["producer", "repeat"]
    );
    lifecycle(&graph, pass, "same", "fixture.loop-leaf", 1)
        .validate_for(&graph)
        .unwrap();
    assert!(graph.body(&["A".into(), "invoke".into()]).is_none());
}

#[test]
fn flow_origins_use_canonical_keys_and_actual_invoker_ids_remain_opaque() {
    let mut graph = graph();
    let definition_key = ScopePathEntry::binding_key("producer", "body");
    let invoker_id = ScopePathEntry::binding_key("not-the-owner", "literal-id");
    for body in &mut graph.loop_bodies {
        if body.path[0] == "producer" {
            body.path[0] = definition_key.clone();
        }
        if body.path == ["A"] {
            body.nodes[1].id = invoker_id.clone();
            body.execution_order[1] = invoker_id.clone();
        }
    }
    graph.validate().unwrap();
    let path = vec![
        ScopePathEntry::Call {
            call_id: "A".into(),
        },
        ScopePathEntry::Flow {
            call_id: invoker_id,
            definition: FlowOrigin {
                workflow_id: graph.workflow_id.clone(),
                path: vec![definition_key.clone()],
            },
        },
    ];
    assert_eq!(
        graph.scope_definition_path(&path).unwrap(),
        [definition_key]
    );
    lifecycle(&graph, path, "same", "fixture.target", 0)
        .validate_for(&graph)
        .unwrap();
}

#[test]
fn flow_scope_validation_checks_actual_callsite_origin_and_callable_body() {
    let graph = graph();
    let mut paths = Vec::new();
    for (id, target) in [
        ("missing", vec!["producer"]),
        ("invoke", vec!["missing"]),
        ("invoke", vec!["producer", "each"]),
        ("invoke", vec!["producer", "repeat"]),
    ] {
        paths.push(vec![
            ScopePathEntry::Call {
                call_id: "A".into(),
            },
            ScopePathEntry::Flow {
                call_id: id.into(),
                definition: origin(&graph, &target),
            },
        ]);
    }
    // The invoker must be in A's definition, not merely exist somewhere in the graph.
    paths.push(vec![invocation(&graph)[1].clone()]);
    let mut foreign = origin(&graph, &["producer"]);
    foreign.workflow_id =
        mf_telemetry::identity::WorkflowId::try_from(format!("sha256:{}", "c".repeat(64))).unwrap();
    paths.push(vec![
        ScopePathEntry::Call {
            call_id: "A".into(),
        },
        ScopePathEntry::Flow {
            call_id: "invoke".into(),
            definition: foreign,
        },
    ]);
    for path in paths {
        assert!(graph.scope_definition_path(&path).is_err(), "{path:?}");
    }
    let mut old = graph.clone();
    old.version = WorkflowDescriptionVersion::V2026_10_23;
    // Kind labels are opaque metadata, not a feature/provenance declaration.
    old.validate().unwrap();
    assert!(old.scope_definition_path(&invocation(&graph)).is_err());
}

#[test]
fn isolated_node_identity_preserves_the_existing_canonical_hash_mapping() {
    let workflow_id = graph().workflow_id;
    let isolated = workflow_id.for_node_target("outer").unwrap();
    assert_eq!(
        isolated.as_str(),
        "sha256:85651631a34af1013a2c0709008af4e7115a13237e6129edfb9b167758b87772"
    );
    let existing = mf_telemetry::identity::WorkflowId::from_definition(
        &json!({"isolation_format": 1, "workflow_id": workflow_id, "target": "outer"}),
        &["outer".into()],
    )
    .unwrap();
    assert_eq!(isolated, existing);
    assert_ne!(isolated, workflow_id.for_node_target("other").unwrap());
    assert_ne!(isolated, workflow_id.for_node_target(" outer ").unwrap());
    assert!(workflow_id.for_node_target(" ").is_err());
}

#[test]
fn isolated_flow_origins_require_exact_operation_provenance_and_retained_callable_bodies() {
    let original = WorkflowDescription::from_json(&serde_json::to_vec(&json!({
        "version":"2026-11-09", "workflow_id":graph().workflow_id,
        "execution":{"mode":"single"},
        "nodes":[{"id":"outer", "kind":"workflow.call"}, {"id":"other", "kind":"builtin.constant"}],
        "execution_order":["outer", "other"], "data_edges":[], "control_edges":[],
        "loop_bodies":[
            {"path":["outer"], "nodes":[{"id":"B", "kind":"builtin.constant"}, {"id":"invoke", "kind":"workflow.call"}, {"id":"each", "kind":"workflow.iteration"}], "execution_order":["B", "invoke", "each"], "data_edges":[], "control_edges":[]},
            {"path":["outer", "each"], "source":"%iteration", "nodes":[{"id":"%iteration", "kind":"%iteration"}, {"id":"invoke", "kind":"workflow.call"}], "execution_order":["%iteration", "invoke"], "data_edges":[], "control_edges":[]},
            {"path":["outer", "B"], "nodes":[{"id":"leaf", "kind":"fixture.target"}, {"id":"nested", "kind":"workflow.call"}], "execution_order":["leaf", "nested"], "data_edges":[], "control_edges":[]},
            {"path":["outer", "B", "nested"], "nodes":[{"id":"leaf", "kind":"fixture.nested"}], "execution_order":["leaf"], "data_edges":[], "control_edges":[]},
            {"path":["other"], "nodes":[{"id":"leaf", "kind":"fixture.other"}], "execution_order":["leaf"], "data_edges":[], "control_edges":[]}
        ]
    })).unwrap()).unwrap();
    let mut isolated = original.clone();
    isolated.workflow_id = original.workflow_id.for_node_target("outer").unwrap();
    isolated.nodes.retain(|node| node.id == "outer");
    isolated.execution_order = vec!["outer".into()];
    isolated.loop_bodies.retain(|body| body.path[0] == "outer");
    isolated.validate().unwrap();
    let target = origin(&original, &["outer", "B"]);
    isolated.validate_flow_definition(&target).unwrap();
    let caller = vec![ScopePathEntry::Call {
        call_id: "outer".into(),
    }];
    let mut path = caller.clone();
    path.push(ScopePathEntry::Flow {
        call_id: "invoke".into(),
        definition: target.clone(),
    });
    assert_eq!(
        isolated.scope_definition_path(&path).unwrap(),
        ["outer", "B"]
    );
    lifecycle(&isolated, path.clone(), "leaf", "fixture.target", 0)
        .validate_for(&isolated)
        .unwrap();
    let mut nested = path.clone();
    nested.push(ScopePathEntry::Call {
        call_id: "nested".into(),
    });
    lifecycle(&isolated, nested, "leaf", "fixture.nested", 0)
        .validate_for(&isolated)
        .unwrap();
    let mut run = Observer::stderr(0)
        .start_with_description(isolated.clone(), RunId::new())
        .unwrap();
    run.validate_flow_origin("invoke", &caller, &target)
        .unwrap();
    assert!(run.begin_invocation(path, "leaf").is_some());
    let iteration = run
        .iteration_observation(
            "each",
            "each",
            &caller,
            vec![NodeIdentity {
                id: "invoke".into(),
                kind: "workflow.call".into(),
                path: vec![],
            }],
        )
        .unwrap();
    let mut item = iteration.begin_item(0).unwrap();
    let body = item.body_observation();
    let nodes = [NodeIdentity {
        id: "leaf".into(),
        kind: "fixture.target".into(),
        path: vec![],
    }];
    let flow_body = body.for_flow_node("invoke", &target, &nodes).unwrap();
    assert!(flow_body.begin_node("leaf").is_some());

    let mut foreign = target.clone();
    foreign.workflow_id =
        mf_telemetry::identity::WorkflowId::try_from(format!("sha256:{}", "c".repeat(64))).unwrap();
    assert!(matches!(
        isolated.validate_flow_definition(&foreign),
        Err(mf_telemetry::ContractError::UnsupportedFlowOrigin { .. })
    ));
    assert!(matches!(
        body.for_flow_node("invoke", &foreign, &nodes),
        Err(mf_telemetry::ContractError::UnsupportedFlowOrigin { .. })
    ));
    let missing = origin(&original, &["other"]);
    assert!(body.for_flow_node("invoke", &missing, &nodes).is_err());
    assert!(
        isolated
            .validate_flow_definition(&missing)
            .unwrap_err()
            .to_string()
            .contains("unknown Flow origin definition")
    );
    item.finish(None);
    drop(iteration);
    run.finish(None);
    let mut wrong_target = isolated.clone();
    wrong_target.workflow_id = original.workflow_id.for_node_target("other").unwrap();
    wrong_target.validate().unwrap();
    assert!(matches!(
        wrong_target.validate_flow_definition(&target),
        Err(mf_telemetry::ContractError::UnsupportedFlowOrigin { .. })
    ));
    let mut unisolated_shape = original.clone();
    unisolated_shape.workflow_id = isolated.workflow_id.clone();
    unisolated_shape.validate().unwrap();
    assert!(matches!(
        unisolated_shape.validate_flow_definition(&target),
        Err(mf_telemetry::ContractError::UnsupportedFlowOrigin { .. })
    ));
    let mut mismatched_order = isolated;
    mismatched_order.execution_order = vec!["other".into()];
    assert!(matches!(
        mismatched_order.validate_flow_definition(&target),
        Err(mf_telemetry::ContractError::UnsupportedFlowOrigin { .. })
    ));
}

#[test]
fn flow_wire_contract_is_strict_versioned_and_preserves_target_history_keys() {
    let graph = graph();
    let path = invocation(&graph);
    let frame = serde_json::to_value(&path[1]).unwrap();
    assert_eq!(
        frame,
        json!({"call_id":"invoke", "definition":{"workflow_id":graph.workflow_id, "path":["producer"]}})
    );
    for invalid in [
        json!({"call_id":"invoke", "definition":frame["definition"], "index":0}),
        json!({"loop_id":"invoke", "index":0, "definition":frame["definition"]}),
        json!({"call_id":"invoke", "definition":{"workflow_id":graph.workflow_id,"path":["producer"],"extra":0}}),
    ] {
        assert!(serde_json::from_value::<ScopePathEntry>(invalid).is_err());
    }
    let mut keys = std::collections::BTreeSet::new();
    keys.insert(path[1].clone());
    keys.insert(ScopePathEntry::Flow {
        call_id: "invoke".into(),
        definition: origin(&graph, &["producer", "nested"]),
    });
    assert_eq!(keys.len(), 2);
    let event = lifecycle(&graph, path, "same", "fixture.target", 0);
    let wire = WireRecord::from_event(&event, 0, None).unwrap();
    assert_eq!(wire.schema_version().unwrap(), FLOW_EVENT_SCHEMA_VERSION);
    assert_eq!(wire.decode().unwrap(), event);
    let mut downgraded = wire.clone();
    downgraded.attributes.insert(
        "mf.schema.version".into(),
        json!(mf_telemetry::EVENT_SCHEMA_VERSION),
    );
    assert!(
        downgraded
            .decode()
            .unwrap_err()
            .to_string()
            .contains("extended event schema")
    );
    let stream = StreamRecord {
        workflow_id: graph.workflow_id.clone(),
        run_id: event.run_id,
        sequence: event.sequence,
        identity: Some(StreamIdentity {
            invocation: 2,
            parent: Some(1),
            trigger: StreamTrigger::Startup,
            message: None,
        }),
        payload: StreamPayload::Execution(event.event),
        emission_count: None,
    };
    let wire = stream.to_wire(0, None).unwrap();
    assert_eq!(
        wire.attributes["mf.schema.version"],
        FLOW_STREAM_EVENT_SCHEMA_VERSION
    );
    assert_eq!(StreamRecord::decode(&wire).unwrap(), stream);
    let boundary = LifecycleEvent {
        workflow_id: graph.workflow_id.clone(),
        run_id: event.run_id,
        sequence: count(1),
        event: Event::WorkflowStarted {
            elapsed_ns: Count::ZERO,
        },
    };
    let wire = WireRecord::from_event_with_schema(&boundary, 0, None, graph.event_schema_version())
        .unwrap();
    assert_eq!(wire.schema_version().unwrap(), FLOW_EVENT_SCHEMA_VERSION);
}

#[test]
fn observed_flow_and_iteration_bodies_use_the_shared_resolver() {
    let graph = graph();
    let mut run = Observer::stderr(0)
        .start_with_description(graph.clone(), RunId::new())
        .unwrap();
    let caller = vec![ScopePathEntry::Call {
        call_id: "A".into(),
    }];
    let target = origin(&graph, &["producer"]);
    run.validate_flow_origin("invoke", &caller, &target)
        .unwrap();
    assert!(
        run.validate_flow_origin("missing", &caller, &target)
            .is_err()
    );
    let path = invocation(&graph);
    assert!(run.begin_invocation(path.clone(), "same").is_some());
    assert!(run.begin_invocation(path.clone(), "missing").is_none());
    let nodes = vec![NodeIdentity {
        id: "invoke".into(),
        kind: "workflow.call".into(),
        path: vec![],
    }];
    let iteration = run
        .iteration_observation("each", "each", &path, nodes)
        .unwrap();
    let mut item = iteration.begin_item(4).unwrap();
    let body = item.body_observation();
    let leaf = NodeIdentity {
        id: "same".into(),
        kind: "fixture.leaf".into(),
        path: vec![],
    };
    let flow = body
        .for_flow_node("invoke", &origin(&graph, &["producer", "nested"]), &[leaf])
        .unwrap();
    assert!(flow.begin_node("same").is_some());
    assert!(body.for_flow_node("missing", &target, &[]).is_err());
    assert!(matches!(
        body.for_flow_node("invoke", &target, &[]),
        Err(mf_telemetry::ContractError::UnsupportedFlowIterationScope { .. })
    ));
    item.finish(None);
    drop(iteration);
    run.finish(None);
}

#[test]
fn iteration_extended_context_retains_item_ancestry_separate_from_flow_location() {
    let graph = graph();
    let detail = IterationDetailContext {
        context: IterationContext {
            invocation: count(1),
            iteration_path: vec!["producer".into(), "each".into()],
            body_path: vec!["producer".into(), "nested".into()],
            parent_path: invocation(&graph),
            parent_stream_invocation: None,
            index: count(3),
            sequence: count(5),
            elapsed_ns: count(100),
        },
        body_scope: vec![ScopePathEntry::Flow {
            call_id: "invoke".into(),
            definition: origin(&graph, &["producer", "nested"]),
        }],
    };
    let encoded = serde_json::to_string(&detail).unwrap();
    assert_eq!(IterationDetailContext::from_json(&encoded).unwrap(), detail);
    assert_eq!(detail.context.item().index, count(3));
    assert!(detail.context.validate().is_err());
    assert_eq!(
        graph
            .scope_definition_path_from(&detail.context.iteration_path, &detail.body_scope)
            .unwrap(),
        detail.context.body_path
    );
}

#[test]
fn dynamic_skip_dependencies_are_checked_in_the_target_not_caller() {
    let mut graph = graph();
    let body = graph
        .loop_bodies
        .iter_mut()
        .find(|body| body.path == ["producer"])
        .unwrap();
    body.order_edges.push(mf_telemetry::description::OrderEdge {
        from_node: "same".into(),
        to_node: "nested".into(),
    });
    let mut event = lifecycle(&graph, invocation(&graph), "nested", "workflow.call", 1);
    let (node, position) = event.event.node().unwrap();
    event.event = Event::NodeSkipped {
        node: node.clone(),
        position,
        elapsed_ns: count(2),
        selected_inputs: vec![],
        causes: vec![SkipCause::NodeSuccess {
            node_success: "same".into(),
        }],
        skipped_ports: vec![],
    };
    event.validate_for(&graph).unwrap();
    if let Event::NodeSkipped { causes, .. } = &mut event.event {
        causes[0] = SkipCause::NodeSuccess {
            node_success: "invoke".into(),
        };
    }
    assert!(event.validate_for(&graph).is_err());
}
