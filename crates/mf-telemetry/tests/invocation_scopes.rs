use mf_telemetry::{
    Count,
    description::{CallTargetDescription, LoopBodyDescription, WorkflowDescription},
    event::{Event, LifecycleEvent, NodeIdentity, ScopePathEntry},
    identity::RunId,
    stream::{StreamIdentity, StreamPayload, StreamRecord, StreamTrigger},
    wire::WireRecord,
};
use serde_json::json;

fn count(value: i64) -> Count {
    Count::try_from(value).unwrap()
}

fn template(
    path: &[&str],
    scope: &WorkflowDescription,
    targets: &[(&str, &str)],
) -> LoopBodyDescription {
    LoopBodyDescription {
        path: path.iter().map(|s| (*s).into()).collect(),
        nodes: scope.nodes.clone(),
        execution_order: scope.execution_order.clone(),
        data_edges: scope.data_edges.clone(),
        control_edges: scope.control_edges.clone(),
        order_edges: scope.order_edges.clone(),
        clusters: vec![],
        call_targets: targets
            .iter()
            .enumerate()
            .map(|(index, (name, target))| CallTargetDescription {
                binding: count(index as i64),
                name: (*name).into(),
                target: (*target).into(),
            })
            .collect(),
    }
}

fn graph() -> WorkflowDescription {
    let mut graph: WorkflowDescription = serde_json::from_value(json!({
        "version":"2026-10-28","workflow_id":format!("sha256:{}", "a".repeat(64)), "execution":{"mode":"single"},
        "nodes":[{"id":"agent","kind":"builtin.llm"},{"id":"tool","kind":"fixture.tool"},{"id":"delegate","kind":"workflow.call"}],
        "execution_order":["agent","tool","delegate"],"data_edges":[],"control_edges":[]
    })).unwrap();
    let mut child = graph.clone();
    child.nodes = serde_json::from_value(
        json!([{"id":"child_agent","kind":"builtin.llm"},{"id":"leaf","kind":"fixture.tool"}]),
    )
    .unwrap();
    child.execution_order = vec!["child_agent".into(), "leaf".into()];
    graph.loop_bodies = vec![
        template(
            &["agent"],
            &graph,
            &[("search", "tool"), ("delegate", "delegate")],
        ),
        template(&["delegate"], &child, &[]),
        template(&["delegate", "child_agent"], &child, &[("leaf", "leaf")]),
    ];
    graph
}

fn invoke(caller: &str, target: &str, binding: i64, invocation: i64) -> ScopePathEntry {
    ScopePathEntry::Invocation {
        call_id: caller.into(),
        target_id: target.into(),
        binding: count(binding),
        invocation: count(invocation),
    }
}

#[test]
fn repeated_invocations_have_distinct_wire_identities_without_loop_index_limits() {
    let graph = graph();
    graph.validate().unwrap();
    assert_eq!(graph.static_node_count().unwrap().get(), 5);
    let first = invoke("agent", "tool", 0, 0);
    let second = invoke(
        "agent",
        "tool",
        0,
        i64::from(mf_telemetry::MAX_LOOP_ITERATIONS) + 1,
    );
    assert_ne!(first, second);
    assert!(second.is_call());
    assert_eq!(second.node_id(), "agent");
    for entry in [first, second] {
        let event = LifecycleEvent {
            workflow_id: graph.workflow_id.clone(),
            run_id: RunId::new(),
            sequence: count(2),
            event: Event::CallStarted {
                path: vec![entry.clone()],
                elapsed_ns: count(1),
            },
        };
        event.validate_for(&graph).unwrap();
        assert_eq!(
            WireRecord::from_event(&event, 0, None)
                .unwrap()
                .decode()
                .unwrap(),
            event
        );
        let stream = StreamRecord {
            workflow_id: graph.workflow_id.clone(),
            run_id: event.run_id,
            sequence: count(2),
            emission_count: None,
            identity: Some(StreamIdentity {
                invocation: 3,
                parent: None,
                trigger: StreamTrigger::Startup,
                message: None,
            }),
            payload: StreamPayload::Execution(event.event),
        };
        assert_eq!(
            StreamRecord::decode(&stream.to_wire(0, None).unwrap()).unwrap(),
            stream
        );
        assert_eq!(
            serde_json::from_value::<ScopePathEntry>(serde_json::to_value(&entry).unwrap())
                .unwrap(),
            entry
        );
    }
}

#[test]
fn scope_resolution_reuses_physical_templates_across_static_and_dynamic_calls() {
    let graph = graph();
    let path = vec![
        invoke("agent", "delegate", 1, 10),
        ScopePathEntry::Call {
            call_id: "delegate".into(),
        },
        invoke("child_agent", "leaf", 0, 11),
    ];
    graph.validate_scope_path(&path).unwrap();
    let body = graph
        .loop_body(&["agent".into(), "delegate".into(), "child_agent".into()])
        .unwrap();
    assert_eq!(body.path, vec!["delegate", "child_agent"]);
    assert_eq!(
        graph.node_in_scope(&path, "leaf").unwrap().kind,
        "fixture.tool"
    );
    assert_eq!(
        graph.node_in_scope(&[], "agent").unwrap().kind,
        "builtin.llm"
    );
    let mut event = LifecycleEvent {
        workflow_id: graph.workflow_id.clone(),
        run_id: RunId::new(),
        sequence: count(4),
        event: Event::NodeStarted {
            selected_inputs: vec![],
            node: NodeIdentity {
                id: "leaf".into(),
                kind: "fixture.tool".into(),
                path,
            },
            position: count(1),
            elapsed_ns: count(2),
        },
    };
    event.validate_for(&graph).unwrap();
    if let Event::NodeStarted { node, position, .. } = &mut event.event {
        node.id = "child_agent".into();
        node.kind = "builtin.llm".into();
        *position = count(0);
    }
    assert!(event.validate_for(&graph).is_err());
    for path in [
        vec![invoke("agent", "delegate", 0, 0)],
        vec![invoke("agent", "unknown", 0, 0)],
        vec![invoke("agent", "tool", 9, 0)],
        vec![invoke("agent", "tool", 0, 0), invoke("agent", "tool", 0, 1)],
        vec![ScopePathEntry::Call {
            call_id: "agent".into(),
        }],
    ] {
        assert!(graph.validate_scope_path(&path).is_err());
    }
}

#[test]
fn templates_require_supported_versions_and_matching_physical_contracts() {
    for scenario in [
        "version", "nodes", "edges", "order", "alias", "id", "target", "path", "kind",
    ] {
        let mut invalid = graph();
        match scenario {
            "version" => invalid.version=mf_telemetry::description::WorkflowDescriptionVersion::V2026_10_23,
            "nodes" => invalid.loop_bodies[0].nodes[1].kind="wrong".into(),
            "edges" => invalid.loop_bodies[0].data_edges=serde_json::from_value(json!([{"from_node":"agent","from_output":"value","to_node":"tool","to_input":"input"}])).unwrap(),
            "order" => invalid.loop_bodies[0].execution_order.reverse(),
            "alias" => invalid.loop_bodies[0].call_targets[0].name="".into(),
            "id" => invalid.loop_bodies[0].call_targets[1].binding=count(0),
            "target" => invalid.loop_bodies[0].call_targets[0].target="missing".into(),
            "path" => { let body=invalid.loop_bodies[0].clone(); invalid.loop_bodies.push(body); },
            "kind" => { invalid.nodes[0].kind="workflow.call".into(); invalid.loop_bodies[0].nodes=invalid.nodes.clone(); },
            _ => unreachable!(),
        }
        assert!(invalid.validate().is_err(), "{scenario}");
    }
    for invalid in [
        json!({"call_id":"agent","binding":0,"target_id":"tool"}),
        json!({"call_id":"agent","binding":-1,"target_id":"tool","invocation":0}),
        json!({"call_id":"agent","binding":0,"target_id":"tool","invocation":0,"extra":true}),
    ] {
        assert!(serde_json::from_value::<ScopePathEntry>(invalid).is_err());
    }
    let event = Event::CallStarted {
        path: vec![invoke("agent", "", 0, 0)],
        elapsed_ns: count(0),
    };
    assert!(event.validate().is_err());
}
