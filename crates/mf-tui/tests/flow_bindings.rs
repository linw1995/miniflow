use mf_runtime::{CompiledGraph, describe_graph};
use mf_telemetry::{description::WorkflowDescription, event::ScopePathEntry};
use mf_tui::{
    graph::{BodyDisplay, ConnectionView, GraphError, GraphLayout, GraphView},
    state::MAX_SESSION_NODES,
};
use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn graph(nodes: Value, order: &[&str]) -> CompiledGraph {
    CompiledGraph {
        definition: serde_json::from_value(json!({
            "version":"2026-10-28", "dependencies":{}, "nodes":nodes
        }))
        .unwrap(),
        execution_order: order.iter().map(|id| (*id).into()).collect(),
        relations: Default::default(),
        children: Default::default(),
    }
}

#[test]
fn authored_binding_shaped_call_ids_keep_their_own_bodies() {
    for nested in [false, true] {
        let static_id = ScopePathEntry::binding_key("invoke", "first");
        let binding_key = ScopePathEntry::binding_key("invoke", "second");
        let parent: Vec<String> = if nested { vec!["outer".into()] } else { vec![] };
        // A binding-shaped physical ID still receives its own encoded child key.
        // Decoding that key once yields static_id, not the unrelated invoke node.
        let static_key = ScopePathEntry::binding_key(&static_id, "body");
        let static_path = [parent.clone(), vec![static_key]].concat();
        let binding_path = [parent.clone(), vec![binding_key]].concat();
        let mut scope = graph(
            json!([
                {"id":"invoke", "kind":"fixture.custom"},
                {"id":static_id, "kind":"fixture.custom"}
            ]),
            &["invoke", &static_id],
        );
        scope.children.insert(
            static_id.clone().into(),
            [(
                "body".into(),
                graph(
                    json!([{"id":"static_leaf", "kind":"fixture.task"}]),
                    &["static_leaf"],
                ),
            )]
            .into(),
        );
        scope.children.insert(
            "invoke".into(),
            [(
                "second".into(),
                graph(
                    json!([{"id":"bound_leaf", "kind":"fixture.task"}]),
                    &["bound_leaf"],
                ),
            )]
            .into(),
        );
        let compiled = if nested {
            let mut root = graph(json!([{"id":"outer", "kind":"workflow.call"}]), &["outer"]);
            scope.relations.key = Some("outer".into());
            root.children
                .insert("outer".into(), [("body".into(), scope)].into());
            root
        } else {
            scope
        };
        // Use the compiled preflight so no ambiguous raw observation paths bypass it.
        let description = describe_graph(&compiled).unwrap();
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
            .map(|body| body.path.clone())
            .collect();
        let layout = GraphLayout::with_expanded_bodies(
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
fn summary_projects_only_outermost_router_bindings_and_keeps_their_paths() {
    let route = ScopePathEntry::binding_key;
    let search = vec!["agent".into(), route("router", "search")];
    let fetch = vec!["agent".into(), route("router", "fetch")];
    let nested = [search.clone(), vec![route("nested", "inner")]].concat();
    let unrelated = vec!["agent".into(), route("helper", "other")];
    let body = |path: &[String], nodes: &[(&str, &str)]| {
        json!({
            "path":path,
            "nodes":nodes.iter().map(|(id, kind)| json!({"id":id,"kind":kind})).collect::<Vec<_>>(),
            "execution_order":nodes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            "data_edges":[],"control_edges":[]
        })
    };
    let description: WorkflowDescription = serde_json::from_value(json!({
        "version":"2026-11-09", "workflow_id":format!("sha256:{}", "b".repeat(64)),
        "execution":{"mode":"single"},
        "nodes":[{"id":"agent","kind":"workflow.call"},{"id":"plain","kind":"workflow.call"}],
        "execution_order":["agent","plain"],"data_edges":[],"control_edges":[],
        // List the nested binding first to exercise path ordering, not input order.
        "loop_bodies":[
            body(&nested, &[("inner_leaf", "fixture.task")]),
            body(&search, &[("leaf", "fixture.task"),("nested", "builtin.router")]),
            body(&fetch, &[("leaf", "fixture.task")]),
            body(&unrelated, &[("hidden_leaf", "fixture.task")]),
            body(&["agent".into()], &[("helper", "fixture.custom"),("router", "builtin.router")]),
            body(&["plain".into()], &[("ordinary_leaf", "fixture.task")]),
        ]
    }))
    .unwrap();
    let mut states: BTreeMap<_, _> = description
        .loop_bodies
        .iter()
        .map(|body| (body.path.clone(), BodyDisplay::Expanded))
        .collect();
    for owner in ["agent", "plain"] {
        states.insert(vec![owner.into()], BodyDisplay::Summary);
    }
    let layout = GraphLayout::with_body_states(
        &description,
        ConnectionView::Execution,
        &states,
        &BTreeMap::new(),
    )
    .unwrap();
    let agent = layout.display_index_for_node_id("agent").unwrap();
    let plain = layout.display_index_for_node_id("plain").unwrap();
    assert_eq!(layout.body_display(agent), Some(BodyDisplay::Summary));
    assert_eq!(layout.body_display(plain), Some(BodyDisplay::Hidden));
    assert!(!layout.can_summarize_body(plain));
    let a = layout
        .display_index_for_scoped_node(&search, "leaf")
        .unwrap();
    let b = layout
        .display_index_for_scoped_node(&fetch, "leaf")
        .unwrap();
    assert_ne!(a, b);
    assert!(
        layout
            .display_index_for_scoped_node(&nested, "inner_leaf")
            .is_some()
    );
    assert_eq!(
        layout
            .nodes()
            .iter()
            .filter(|node| node.id == "inner_leaf")
            .count(),
        1
    );
    assert!(
        layout
            .display_index_for_scoped_node(&unrelated, "hidden_leaf")
            .is_none()
    );
    assert!(
        layout
            .display_index_for_scoped_node(&["agent".into()], "router")
            .is_none()
    );
    let (width, height) = layout.size();
    let area = Rect::new(0, 0, width as u16, height as u16);
    let mut buffer = Buffer::empty(area);
    GraphView::new(&layout).render(area, &mut buffer);
    let text: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("agent [~]"), "{text}");
    assert!(text.contains("plain [+]"), "{text}");
    for label in ["Route: search", "Route: fetch"] {
        assert_eq!(text.matches(label).count(), 1, "{label}");
    }
    assert!(!text.contains("Route: inner"));
    assert!(!text.contains("Route: other"));
}

#[test]
fn router_half_fold_keeps_route_nodes_edges_and_nested_fold_state() {
    let first = vec![ScopePathEntry::binding_key("execute", "first")];
    let second = vec![ScopePathEntry::binding_key("execute", "second")];
    let nested = [
        first.clone(),
        vec![ScopePathEntry::binding_key("inner", "tool")],
    ]
    .concat();
    let description: WorkflowDescription = serde_json::from_value(json!({
        "version":"2026-11-09", "workflow_id":format!("sha256:{}", "f".repeat(64)),
        "execution":{"mode":"single"}, "nodes":[{"id":"execute","kind":"builtin.router"}],
        "execution_order":["execute"], "data_edges":[], "control_edges":[],
        "loop_bodies":[
            {"path":first, "nodes":[{"id":"leaf","kind":"fixture.task"},{"id":"inner","kind":"builtin.router"}],
             "execution_order":["leaf","inner"], "order_edges":[{"from_node":"leaf","to_node":"inner"}], "data_edges":[], "control_edges":[]},
            {"path":second, "nodes":[{"id":"leaf","kind":"fixture.task"}], "execution_order":["leaf"], "data_edges":[], "control_edges":[]},
            {"path":nested, "nodes":[{"id":"nested_leaf","kind":"fixture.task"}], "execution_order":["nested_leaf"], "data_edges":[], "control_edges":[]}
        ]
    }))
    .unwrap();
    // A request to expand the Router opens the same graph as its default half-fold.
    for mode in [BodyDisplay::Summary, BodyDisplay::Expanded] {
        let layout = GraphLayout::with_body_states(
            &description,
            ConnectionView::Execution,
            &[(first.clone(), mode), (second.clone(), mode)].into(),
            &BTreeMap::new(),
        )
        .unwrap();
        let execute = layout.display_index_for_node_id("execute").unwrap();
        assert_eq!(layout.body_display(execute), Some(BodyDisplay::Summary));
        let inner = layout
            .display_index_for_scoped_node(&first, "inner")
            .unwrap();
        assert_eq!(layout.body_display(inner), Some(BodyDisplay::Hidden));
        assert!(
            layout
                .display_index_for_scoped_node(&nested, "nested_leaf")
                .is_none()
        );
        let a = layout
            .display_index_for_scoped_node(&first, "leaf")
            .unwrap();
        let b = layout
            .display_index_for_scoped_node(&second, "leaf")
            .unwrap();
        assert_ne!(a, b);
        assert_eq!(layout.nodes()[a].id, "leaf");
        assert_eq!(layout.nodes()[b].id, "leaf");
        let edges = layout.edges();
        assert_eq!(edges.len(), 1);
        assert_eq!((edges[0].from, edges[0].to), (a, inner));
        let (width, height) = layout.size();
        let area = Rect::new(0, 0, width as u16, height as u16);
        let mut buffer = Buffer::empty(area);
        GraphView::new(&layout).render(area, &mut buffer);
        let text: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("execute [~]"), "{text}");
        assert!(text.contains("inner [+]"), "{text}");
    }
}

#[test]
fn open_binding_bodies_share_the_node_budget_with_their_owner() {
    let path = vec![ScopePathEntry::binding_key("invoke", "provided")];
    // Each scope fits individually, but the owner plus its body exceeds the display budget.
    let nodes: Vec<_> = (0..MAX_SESSION_NODES)
        .map(|index| json!({"id":format!("leaf-{index}"), "kind":"fixture.task"}))
        .collect();
    let order: Vec<_> = nodes.iter().map(|node| node["id"].clone()).collect();
    let mut description: WorkflowDescription = serde_json::from_value(json!({
        "version":"2026-10-23", "workflow_id":format!("sha256:{}", "b".repeat(64)),
        "execution":{"mode":"single"},
        "nodes":[{"id":"invoke", "kind":"fixture.custom"}], "execution_order":["invoke"],
        "data_edges":[], "control_edges":[],
        "loop_bodies":[{
            "path":path, "nodes":nodes, "execution_order":order,
            "data_edges":[], "control_edges":[]
        }]
    }))
    .unwrap();
    for (kind, mode) in [
        ("fixture.custom", BodyDisplay::Expanded),
        ("builtin.router", BodyDisplay::Summary),
    ] {
        description.nodes[0].kind = kind.into();
        let layout = |states| {
            GraphLayout::with_body_states(
                &description,
                ConnectionView::Execution,
                &states,
                &BTreeMap::new(),
            )
        };
        assert_eq!(layout(BTreeMap::new()).unwrap().nodes().len(), 1);
        assert!(matches!(
            layout([(path.clone(), mode)].into()),
            Err(GraphError::TooManyNodes { limit }) if limit == MAX_SESSION_NODES
        ));
    }
}

#[test]
fn passed_binding_bodies_share_one_callsite_without_sharing_node_identity() {
    let paths: Vec<_> = ["first", "second"]
        .map(|binding| vec![ScopePathEntry::binding_key("invoke", binding)])
        .into();
    let description: WorkflowDescription = serde_json::from_value(json!({
        "version":"2026-10-23", "workflow_id":format!("sha256:{}", "b".repeat(64)), "execution":{"mode":"single"},
        "nodes":[{"id":"invoke", "kind":"fixture.custom"}], "execution_order":["invoke"],
        "data_edges":[], "control_edges":[],
        "loop_bodies":paths.iter().map(|path| json!({
            "path":path, "nodes":[{"id":"leaf", "kind":"fixture.task"}], "execution_order":["leaf"],
            "data_edges":[], "control_edges":[]
        })).collect::<Vec<_>>()
    })).unwrap();
    description.validate().unwrap();
    for path in &paths {
        description
            .validate_scope_path(&[ScopePathEntry::Call {
                call_id: path[0].clone(),
            }])
            .unwrap();
    }
    let states = paths.iter().cloned().collect();
    let expanded = GraphLayout::with_expanded_bodies(
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
    let collapsed = GraphLayout::with_expanded_bodies(
        &description,
        ConnectionView::Execution,
        &Default::default(),
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
