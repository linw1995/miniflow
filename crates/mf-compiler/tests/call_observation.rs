#[path = "fixtures/observation_capture.rs"]
mod capture;

use capture::Harness;
use mf_telemetry::{
    description::WorkflowDescription,
    event::{Event, FailurePhase, ScopePathEntry},
    identity::RunId,
};
use serde_json::json;

fn graph(empty: bool) -> WorkflowDescription {
    serde_json::from_value(json!({
        "version":"2026-10-23", "workflow_id":format!("sha256:{}", "a".repeat(64)),
        "execution":{"mode":"single"}, "nodes":[{"id":"call", "kind":"workflow.call"}],
        "execution_order":["call"], "data_edges":[], "control_edges":[],
        "loop_bodies":[{
            "path":["call"], "nodes":[{"id":"inner", "kind":"workflow.call"}],
            "execution_order":["inner"], "data_edges":[], "control_edges":[]
        }, {
            "path":["call", "inner"],
            "nodes":if empty { json!([]) } else { json!([{"id":"leaf", "kind":"fixture.task"}]) },
            "execution_order":if empty { json!([]) } else { json!(["leaf"]) },
            "data_edges":[], "control_edges":[]
        }]
    }))
    .unwrap()
}

#[test]
fn nested_calls_use_native_span_parentage_and_preserve_unsampled_lifecycles() {
    for (sampled, empty, failed) in [
        (true, false, false),
        (true, false, true),
        (true, true, false),
        (false, false, false),
    ] {
        let harness = Harness::new(sampled);
        let graph = graph(empty);
        let mut run = harness
            .observer()
            .start_with_description(graph.clone(), RunId::new())
            .unwrap();
        let _run_guard = run.enter();
        let outer_path = vec![ScopePathEntry::Call {
            call_id: "call".into(),
        }];
        let mut inner_path = outer_path.clone();
        inner_path.push(ScopePathEntry::Call {
            call_id: "inner".into(),
        });
        let mut outer = run.begin_node("call").unwrap();
        run.node_started(&mut outer);
        let outer_guard = outer.enter();
        run.call_started(outer_path.clone());
        let mut inner = run.begin_invocation(outer_path.clone(), "inner").unwrap();
        run.node_started(&mut inner);
        let inner_guard = inner.enter();
        run.call_started(inner_path.clone());
        if !empty {
            let mut leaf = run.begin_invocation(inner_path.clone(), "leaf").unwrap();
            run.node_started(&mut leaf);
            if failed {
                run.node_failed(leaf, FailurePhase::Execution, "child failed".into());
            } else {
                run.node_succeeded(leaf, vec![], vec![]);
            }
        }
        run.call_finished(inner_path.clone(), failed);
        drop(inner_guard);
        if failed {
            run.node_failed(inner, FailurePhase::Execution, "child failed".into());
        } else {
            run.node_succeeded(inner, vec![], vec![]);
        }
        run.call_finished(outer_path.clone(), failed);
        drop(outer_guard);
        if failed {
            run.node_failed(outer, FailurePhase::Execution, "call failed".into());
            run.finish(Some(&"call failed"));
        } else {
            run.node_succeeded(outer, vec![], vec![]);
            run.finish(None);
        }
        let events: Vec<_> = harness
            .records()
            .into_iter()
            .map(|record| record.decode().unwrap())
            .collect();
        for event in &events {
            event.validate_for(&graph).unwrap();
        }
        let boundaries: Vec<_> = events
            .iter()
            .filter_map(|record| match &record.event {
                Event::CallStarted { path, .. } => Some(("start", path.clone())),
                Event::CallFinished { path, .. } => Some(("finish", path.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            boundaries,
            [
                ("start", outer_path.clone()),
                ("start", inner_path.clone()),
                ("finish", inner_path.clone()),
                ("finish", outer_path),
            ]
        );
        assert_eq!(
            events
                .iter()
                .filter(|record| matches!(&record.event,
                    Event::NodeStarted { node, .. } if node.id == "leaf" && node.path == inner_path
                ))
                .count(),
            usize::from(!empty)
        );
        let spans = harness.spans.get_finished_spans().unwrap();
        if !sampled {
            assert!(spans.is_empty());
            continue;
        }
        let node_span = |id: &str| {
            spans
                .iter()
                .find(|span| {
                    span.attributes.iter().any(|attribute| {
                        attribute.key.as_str() == "mf.node.id" && attribute.value.as_str() == id
                    })
                })
                .unwrap()
        };
        let outer = node_span("call");
        let inner = node_span("inner");
        assert_eq!(outer.name, "mf.node");
        assert_eq!(inner.name, "mf.node");
        assert_eq!(inner.parent_span_id, outer.span_context.span_id());
        if !empty {
            let leaf = node_span("leaf");
            assert_eq!(leaf.name, "mf.node");
            assert_eq!(leaf.parent_span_id, inner.span_context.span_id());
        }
    }
}
