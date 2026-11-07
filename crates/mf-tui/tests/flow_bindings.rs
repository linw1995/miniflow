use mf_runtime::{CompiledGraph, describe_graph};
use mf_telemetry::{description::WorkflowDescription, event::ScopePathEntry};
use mf_tui::{
    graph::{ConnectionView, GraphError, GraphLayout},
    state::MAX_SESSION_NODES,
};
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
        let static_id = ScopePathEntry::binding_key("invoke", "/first");
        let binding_key = ScopePathEntry::binding_key("invoke", "/second");
        let parent: Vec<String> = if nested { vec!["outer".into()] } else { vec![] };
        // A binding-shaped physical ID still receives its own encoded child key.
        // Decoding that key once yields static_id, not the unrelated invoke node.
        let static_key = ScopePathEntry::binding_key(&static_id, "/body");
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
                "/body".into(),
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
                "/second".into(),
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
                .insert("outer".into(), [("/body".into(), scope)].into());
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
fn expanded_binding_body_propagates_the_shared_node_budget_error() {
    let path = vec![ScopePathEntry::binding_key("invoke", "/provided")];
    // Each scope fits individually, but the owner plus its body exceeds the display budget.
    let nodes: Vec<_> = (0..MAX_SESSION_NODES)
        .map(|index| json!({"id":format!("leaf-{index}"), "kind":"fixture.task"}))
        .collect();
    let order: Vec<_> = nodes.iter().map(|node| node["id"].clone()).collect();
    let description: WorkflowDescription = serde_json::from_value(json!({
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
    description.validate().unwrap();
    let collapsed = GraphLayout::with_expanded_bodies(
        &description,
        ConnectionView::Execution,
        &Default::default(),
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(collapsed.nodes().len(), 1);
    assert!(matches!(
        GraphLayout::with_expanded_bodies(
            &description,
            ConnectionView::Execution,
            &[path].into(),
            &BTreeMap::new(),
        ),
        Err(GraphError::TooManyNodes { limit }) if limit == MAX_SESSION_NODES
    ));
}

#[test]
fn nested_map_and_root_flow_pointers_keep_distinct_projection_identities() {
    let body = json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[{"id":"leaf", "kind":"fixture.task"}]
    });
    let mut compiled = graph(
        json!([{"id":"invoke", "kind":"fixture.custom", "config":{
            "policies":{"a/b":{"body":body}, "a~b":{"body":body}},
            "branches":[{"body":body}],
            "decoy":body
        }}]),
        &["invoke"],
    );
    let pointers = [
        "/policies/a~1b/body",
        "/policies/a~0b/body",
        "/branches/0/body",
    ];
    compiled.children.insert(
        "invoke".into(),
        pointers
            .map(|pointer| {
                (
                    pointer.into(),
                    graph(json!([{"id":"leaf", "kind":"fixture.task"}]), &["leaf"]),
                )
            })
            .into(),
    );
    // The compiled children, not Flow-shaped configuration or providers, authorize bodies.
    let mut description = describe_graph(&compiled).unwrap();
    let mut paths = pointers
        .map(|pointer| vec![ScopePathEntry::binding_key("invoke", pointer)])
        .to_vec();
    assert_eq!(description.loop_bodies.len(), pointers.len());
    assert!(
        description
            .body(&[ScopePathEntry::binding_key("invoke", "/decoy")])
            .is_none()
    );

    // A custom node whose entire configuration is a Flow uses the empty JSON pointer.
    let mut root = graph(
        json!([{"id":"root", "kind":"fixture.root-flow", "config":compiled.definition}]),
        &["root"],
    );
    root.children
        .insert("root".into(), [(String::new(), compiled)].into());
    description = describe_graph(&root).unwrap();
    let root_key = ScopePathEntry::binding_key("root", "");
    assert!(!root_key.is_empty());
    for path in &mut paths {
        path.insert(0, root_key.clone());
    }
    description.validate().unwrap();
    assert_eq!(description.loop_bodies.len(), pointers.len() + 1);
    for path in &paths {
        description
            .validate_scope_path(
                &path
                    .iter()
                    .map(|key| ScopePathEntry::Call {
                        call_id: key.clone(),
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    }
    let mut expanded = paths
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    expanded.insert(vec![root_key]);
    let layout = GraphLayout::with_expanded_bodies(
        &description,
        ConnectionView::Execution,
        &expanded,
        &BTreeMap::new(),
    )
    .unwrap();
    let indices: std::collections::BTreeSet<_> = paths
        .iter()
        .map(|path| layout.display_index_for_scoped_node(path, "leaf").unwrap())
        .collect();
    assert_eq!(indices.len(), pointers.len());
    assert_eq!(
        layout
            .nodes()
            .iter()
            .filter(|node| node.id == "leaf")
            .count(),
        pointers.len()
    );
}

#[test]
fn passed_binding_bodies_share_one_callsite_without_sharing_node_identity() {
    let paths: Vec<_> = ["/first", "/second"]
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
