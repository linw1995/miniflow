#[path = "fixtures/observation_capture.rs"]
mod capture;

use capture::Harness;
use mf_telemetry::{
    Count,
    description::WorkflowDescription,
    event::{Event, FailurePhase, NodeIdentity, ScopePathEntry},
    identity::RunId,
};
use serde_json::json;

fn graph() -> WorkflowDescription {
    serde_json::from_value(json!({
        "version":"2026-10-23", "workflow_id":format!("sha256:{}", "a".repeat(64)),
        "execution":{"mode":"single"}, "nodes":[{"id":"call", "kind":"workflow.call"}],
        "execution_order":["call"], "data_edges":[], "control_edges":[],
        "loop_bodies":[{
            "path":["call"], "nodes":[{"id":"leaf", "kind":"fixture.task"}],
            "execution_order":["leaf"], "data_edges":[], "control_edges":[]
        }]
    }))
    .unwrap()
}

#[test]
fn observed_call_lifecycles_validate_without_an_execution_engine() {
    for failed in [false, true] {
        let harness = Harness::new(true);
        let graph = graph();
        let mut run = harness
            .observer()
            .start_with_description(graph.clone(), RunId::new())
            .unwrap();
        let path = vec![ScopePathEntry::Call {
            call_id: "call".into(),
        }];
        let mut root = run.begin_node("call").unwrap();
        run.node_started(&mut root);
        let guard = root.enter();
        run.call_started(path.clone());
        let mut child = run.begin_invocation(path.clone(), "leaf").unwrap();
        run.node_started(&mut child);
        if failed {
            run.node_failed(child, FailurePhase::Execution, "child failed".into());
        } else {
            run.node_succeeded(child, vec![], vec![]);
        }
        run.call_finished(path, failed);
        drop(guard);
        if failed {
            run.node_failed(root, FailurePhase::Execution, "call failed".into());
            run.finish(Some(&"call failed"));
        } else {
            run.node_succeeded(root, vec![], vec![]);
            run.finish(None);
        }
        let events: Vec<_> = harness
            .records()
            .into_iter()
            .map(|record| record.decode().unwrap())
            .collect();
        assert!(
            events
                .iter()
                .any(|event| matches!(event.event, Event::CallStarted { .. }))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event.event, Event::CallFinished { .. }))
        );
        assert!(!events.iter().any(|event| matches!(
            event.event,
            Event::LoopPassStarted { .. } | Event::LoopPassFinished { .. }
        )));
        for event in events {
            event.validate_for(&graph).unwrap();
        }
    }
}

#[test]
fn iteration_body_call_details_retain_nested_paths_and_item_identity() {
    let harness = Harness::new(true);
    let graph = graph();
    let mut run = harness
        .observer()
        .start(
            graph.workflow_id,
            RunId::new(),
            vec![NodeIdentity {
                id: "items".into(),
                kind: "builtin.iteration".into(),
                path: vec![],
            }],
        )
        .unwrap();
    let mut root = run.begin_node("items").unwrap();
    run.node_started(&mut root);
    let nodes = vec![NodeIdentity {
        id: "leaf".into(),
        kind: "fixture.task".into(),
        path: vec![],
    }];
    let iteration = run
        .iteration_observation("items", &[], nodes.clone())
        .unwrap();
    let mut item = iteration.begin_item(3).unwrap();
    let guard = item.enter();
    let outer = item.body_observation().for_call("outer", &nodes);
    let inner = outer.for_call("inner", &nodes);
    assert!(inner.begin_node("unknown").is_none());
    for body in [outer, inner] {
        let mut success = body.begin_node("leaf").unwrap();
        success.started();
        success.succeeded(vec!["value".into()], vec![]);
        body.begin_node("leaf")
            .unwrap()
            .skipped(vec![], vec!["value".into()]);
        body.begin_node("leaf")
            .unwrap()
            .failed(FailurePhase::Execution, "failed".into());
    }
    drop(guard);
    item.finish(None);
    run.node_succeeded(root, vec![], vec![]);
    run.finish(None);
    let logs: Vec<_> = harness
        .records()
        .into_iter()
        .filter(|record| record.event_name.starts_with("mf.call.node."))
        .collect();
    assert_eq!(logs.len(), 8);
    let mut depths = Vec::new();
    for log in logs {
        let path: Vec<ScopePathEntry> =
            serde_json::from_str(log.attributes["mf.scope.path"].as_str().unwrap()).unwrap();
        assert!(path.iter().all(ScopePathEntry::is_call));
        assert_eq!(path[0].node_id(), "outer");
        if path.len() == 2 {
            assert_eq!(path[1].node_id(), "inner");
        }
        depths.push(path.len());
        assert_eq!(log.attributes["mf.iteration.id"], json!("items"));
        assert_eq!(log.attributes["mf.iteration.index"], json!(3));
    }
    assert!(depths.contains(&1) && depths.contains(&2));
    assert!(
        harness
            .spans
            .get_finished_spans()
            .unwrap()
            .iter()
            .any(|span| span.name == "mf.call.node")
    );
}

#[test]
fn iteration_details_with_loop_paths_keep_their_existing_span_kind() {
    let harness = Harness::new(true);
    let mut run = harness
        .observer()
        .start(
            graph().workflow_id,
            RunId::new(),
            vec![NodeIdentity {
                id: "items".into(),
                kind: "builtin.iteration".into(),
                path: vec![],
            }],
        )
        .unwrap();
    let path = vec![ScopePathEntry::Loop {
        loop_id: "enclosing".into(),
        index: Count::ZERO,
    }];
    let iteration = run
        .iteration_observation(
            "items",
            &[],
            vec![NodeIdentity {
                id: "leaf".into(),
                kind: "fixture.task".into(),
                path: path.clone(),
            }],
        )
        .unwrap();
    let mut item = iteration.begin_item(0).unwrap();
    let mut node = item.body_observation().begin_node("leaf").unwrap();
    node.started();
    node.succeeded(vec![], vec![]);
    item.finish(None);
    run.finish(None);
    let records = harness.records();
    assert!(
        records
            .iter()
            .any(|record| record.event_name == "mf.iteration.node.started")
    );
    assert!(
        !records
            .iter()
            .any(|record| record.event_name.starts_with("mf.call.node."))
    );
    let record = records
        .iter()
        .find(|record| record.event_name == "mf.iteration.node.finished")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<ScopePathEntry>>(
            record.attributes["mf.scope.path"].as_str().unwrap()
        )
        .unwrap(),
        path
    );
    assert!(
        harness
            .spans
            .get_finished_spans()
            .unwrap()
            .iter()
            .any(|span| span.name == "mf.iteration.node")
    );
}
