use mf_telemetry::{description::WorkflowDescription, event::ScopePathEntry};
use mf_tui::graph::{BodyDisplay, ConnectionView, GraphLayout};
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn authored_binding_shaped_call_ids_keep_their_own_bodies() {
    for nested in [false, true] {
        let static_id = ScopePathEntry::binding_key("invoke", "first");
        let binding_key = ScopePathEntry::binding_key("invoke", "second");
        let parent: Vec<String> = if nested { vec!["outer".into()] } else { vec![] };
        let static_path = [parent.clone(), vec![static_id.clone()]].concat();
        let binding_path = [parent.clone(), vec![binding_key]].concat();
        let scope_nodes = json!([
            {"id":"invoke", "kind":"workflow.call"},
            {"id":static_id, "kind":"workflow.call"}
        ]);
        let scope_order = json!(["invoke", static_id]);
        let mut bodies = vec![
            json!({
                "path":static_path, "nodes":[{"id":"static_leaf", "kind":"fixture.task"}],
                "execution_order":["static_leaf"], "data_edges":[], "control_edges":[]
            }),
            json!({
                "path":binding_path, "nodes":[{"id":"bound_leaf", "kind":"fixture.task"}],
                "execution_order":["bound_leaf"], "data_edges":[], "control_edges":[]
            }),
        ];
        if nested {
            bodies.push(json!({
                "path":parent, "nodes":scope_nodes, "execution_order":scope_order,
                "data_edges":[], "control_edges":[]
            }));
        }
        let description: WorkflowDescription = serde_json::from_value(json!({
            "version":"2026-10-23", "workflow_id":format!("sha256:{}", "b".repeat(64)),
            "execution":{"mode":"single"},
            "nodes":if nested { json!([{"id":"outer", "kind":"workflow.call"}]) } else { scope_nodes },
            "execution_order":if nested { json!(["outer"]) } else { scope_order },
            "data_edges":[], "control_edges":[], "loop_bodies":bodies,
            "injected_bodies":[binding_path]
        })).unwrap();
        description.validate().unwrap();
        assert_eq!(
            description.body_paths_for_node(&parent, "invoke"),
            vec![&binding_path]
        );
        assert_eq!(
            description.body_paths_for_node(&parent, &static_id),
            vec![&static_path]
        );

        let states = description
            .loop_bodies
            .iter()
            .map(|body| (body.path.clone(), BodyDisplay::Expanded))
            .collect();
        let layout = GraphLayout::with_body_states(
            &description,
            ConnectionView::Execution,
            &states,
            &BTreeMap::new(),
        )
        .unwrap();
        for (path, leaf) in [(&static_path, "static_leaf"), (&binding_path, "bound_leaf")] {
            assert!(layout.display_index_for_scoped_node(path, leaf).is_some());
            assert_eq!(
                layout.nodes().iter().filter(|node| node.id == leaf).count(),
                1
            );
        }
    }
}

#[test]
fn passed_binding_bodies_share_one_callsite_without_sharing_node_identity() {
    let paths: Vec<_> = ["first", "second"]
        .map(|binding| vec![ScopePathEntry::binding_key("invoke", binding)])
        .into();
    let description: WorkflowDescription = serde_json::from_value(json!({
        "version":"2026-10-23", "workflow_id":format!("sha256:{}", "b".repeat(64)), "execution":{"mode":"single"},
        "nodes":[{"id":"invoke", "kind":"workflow.call"}], "execution_order":["invoke"],
        "data_edges":[], "control_edges":[],
        "injected_bodies":paths,
        "loop_bodies":paths.iter().map(|path| json!({
            "path":path, "nodes":[{"id":"leaf", "kind":"fixture.task"}], "execution_order":["leaf"],
            "data_edges":[], "control_edges":[]
        })).collect::<Vec<_>>()
    })).unwrap();
    description.validate().unwrap();
    for binding in ["first", "second"] {
        description
            .validate_scope_path(&[ScopePathEntry::CallBinding {
                call_id: "invoke".into(),
                binding: binding.into(),
            }])
            .unwrap();
    }
    let states = paths
        .iter()
        .map(|path| (path.clone(), BodyDisplay::Expanded))
        .collect();
    let expanded = GraphLayout::with_body_states(
        &description,
        ConnectionView::Execution,
        &states,
        &BTreeMap::new(),
    )
    .unwrap();
    let first = expanded
        .display_index_for_scoped_node(&paths[0], "leaf")
        .unwrap();
    let second = expanded
        .display_index_for_scoped_node(&paths[1], "leaf")
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(
        expanded
            .nodes()
            .iter()
            .filter(|node| node.id == "invoke")
            .count(),
        1
    );
    let hidden = paths
        .iter()
        .map(|path| (path.clone(), BodyDisplay::Hidden))
        .collect();
    let collapsed = GraphLayout::with_body_states(
        &description,
        ConnectionView::Execution,
        &hidden,
        &BTreeMap::new(),
    )
    .unwrap();
    assert!(
        collapsed
            .display_index_for_scoped_node(&paths[0], "leaf")
            .is_none()
    );
    assert_eq!(
        collapsed.display_index_for_scoped_node(&[], &paths[0][0]),
        collapsed.display_index_for_scoped_node(&[], "invoke")
    );
}
