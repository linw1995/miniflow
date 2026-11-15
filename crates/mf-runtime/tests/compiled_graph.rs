use mf_runtime::{
    CompiledGraph, GraphRelations, NodeOperation, ScopePathEntry, ScopeRelations, StateTarget,
    ValueType,
};
use mf_telemetry::identity::WorkflowId;
use serde_json::json;

fn graph() -> CompiledGraph {
    CompiledGraph {
        definition: serde_json::from_value(json!({
            "version": "2026-10-28", "dependencies": {},
            "nodes": [
                {"id": "origin", "kind": "user.noop"},
                {"id": "store", "kind": "user.noop"},
                {"id": "stop", "kind": "user.noop"},
                {"id": "%loop", "kind": "%loop"}
            ]
        }))
        .unwrap(),
        execution_order: ["origin", "store", "stop", "%loop"].map(Into::into).into(),
        relations: GraphRelations::default(),
        children: Default::default(),
    }
}

fn child(graph: &mut CompiledGraph) -> &mut CompiledGraph {
    graph
        .children
        .get_mut(&"origin".into())
        .unwrap()
        .get_mut("selected")
        .unwrap()
}

fn scope() -> ScopeRelations {
    ScopeRelations {
        input: "origin".into(),
        values: [("counter".into(), ValueType::String)].into(),
        state_outputs: [("counter".into(), "counter".into())].into(),
    }
}

fn bind_scope(graph: &mut CompiledGraph) {
    graph.relations.scope = Some(scope());
    for node in &mut graph.definition.nodes {
        node.operation = match node.id.as_str() {
            "origin" => Some(NodeOperation::Input),
            "store" => Some(NodeOperation::Assign {
                target: StateTarget::Frame {
                    field: "counter".into(),
                },
            }),
            "stop" => Some(NodeOperation::Exit),
            _ => None,
        };
    }
}

#[test]
fn node_operations_are_owned_by_nodes_and_do_not_follow_kind_labels_or_scope_links() {
    let mut graph = graph();
    bind_scope(&mut graph);
    for kind in ["user.noop", "renamed.provider", "builtin.iteration_input"] {
        for node in &mut graph.definition.nodes {
            node.kind = kind.into();
        }
        assert_eq!(
            graph.definition.nodes[0].operation,
            Some(NodeOperation::Input)
        );
        assert_eq!(
            graph.definition.nodes[1].operation,
            Some(NodeOperation::Assign {
                target: StateTarget::Frame {
                    field: "counter".into()
                }
            })
        );
        assert_eq!(
            graph.definition.nodes[2].operation,
            Some(NodeOperation::Exit)
        );
        assert_eq!(graph.definition.nodes[3].operation, None);
        mf_runtime::validate_node_operations(&graph).unwrap();
    }
    graph.relations.scope.as_mut().unwrap().input = "another-source".into();
    assert_eq!(
        graph.definition.nodes[0].operation,
        Some(NodeOperation::Input)
    );
    assert!(mf_runtime::validate_node_operations(&graph).is_err());
    graph.relations.scope = None;
    assert!(graph.definition.nodes[1].operation.is_some());
    assert!(mf_runtime::validate_node_operations(&graph).is_err());
}

#[test]
fn callable_and_stateful_depend_only_on_scope_and_state_outputs() {
    let mut relations = GraphRelations::default();
    assert!(relations.is_callable());
    relations.host = Some("opaque-host".into());
    assert!(relations.is_callable());
    relations.scope = Some(scope());
    assert!(!relations.is_callable());
    let scope = relations.scope.as_mut().unwrap();
    assert!(scope.is_stateful());
    scope.state_outputs.clear();
    assert!(!scope.is_stateful());
    assert!(!scope.values.is_empty());
}

#[test]
fn child_scope_keys_use_explicit_override_or_generic_binding_for_every_kind() {
    let mut child = graph();
    let mut owner = child.definition.nodes[0].clone();
    owner.id = "owner/with~punctuation".into();
    for kind in [
        "workflow.call",
        "loop.run",
        "builtin.iteration",
        "user.renamed",
    ] {
        owner.kind = kind.into();
        for binding in ["body", "other"] {
            assert_eq!(
                child.child_scope_key(&owner, binding),
                ScopePathEntry::binding_key(owner.id.as_str(), binding)
            );
        }
    }
    child.relations.key = Some("explicit-edge-identity".into());
    assert_eq!(
        child.child_scope_key(&owner, "body"),
        "explicit-edge-identity"
    );
    assert_eq!(
        child.child_scope_key(&owner, "other"),
        "explicit-edge-identity"
    );
}

#[test]
fn relations_are_required_and_empty_relationship_fields_are_optional() {
    let graph = graph();
    let mut encoded = serde_json::to_value(&graph).unwrap();
    assert_eq!(encoded["relations"], json!({}));
    assert_eq!(
        serde_json::from_value::<CompiledGraph>(encoded.clone()).unwrap(),
        graph
    );
    encoded.as_object_mut().unwrap().remove("relations");
    assert!(serde_json::from_value::<CompiledGraph>(encoded).is_err());
    assert_eq!(
        serde_json::from_value::<GraphRelations>(json!({})).unwrap(),
        GraphRelations::default()
    );
    for field in ["unknown", "writes", "node_writes"] {
        assert!(serde_json::from_value::<GraphRelations>(json!({field:{}})).is_err());
    }
    let encoded = serde_json::to_value(scope()).unwrap();
    for field in ["input", "values", "state_outputs"] {
        let mut missing = encoded.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<ScopeRelations>(missing).is_err());
    }
    for field in ["unknown", "writes", "exits"] {
        let mut invalid = encoded.clone();
        invalid[field] = json!({});
        assert!(serde_json::from_value::<ScopeRelations>(invalid).is_err());
    }
}

#[test]
fn all_concrete_relationships_and_node_operations_round_trip() {
    let mut graph = graph();
    bind_scope(&mut graph);
    graph.relations.key = Some("edge-key".into());
    graph.relations.streams.insert(
        "events".into(),
        serde_json::from_value(json!({"node": "origin", "port": "counter"})).unwrap(),
    );
    assert_eq!(
        serde_json::from_slice::<CompiledGraph>(&serde_json::to_vec(&graph).unwrap()).unwrap(),
        graph
    );
    let encoded = serde_json::to_value(&graph).unwrap();
    assert_eq!(
        encoded["definition"]["nodes"][1]["operation"],
        json!({
            "op":"assign", "target":{"scope":"frame", "field":"counter"}
        })
    );
    assert_eq!(encoded["definition"]["nodes"][1]["config"], json!({}));
    assert!(encoded["relations"].get("writes").is_none());
}

#[test]
fn only_plain_child_free_graphs_retain_definition_order_identity() {
    let mut graph = graph();
    let order = graph
        .execution_order
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let historical = WorkflowId::from_definition(&graph.definition, &order).unwrap();
    assert_eq!(graph.workflow_id().unwrap(), historical);
    bind_scope(&mut graph);
    assert_ne!(graph.workflow_id().unwrap(), historical);
    assert_eq!(
        graph.workflow_id().unwrap(),
        WorkflowId::from_definition(&graph, &order).unwrap()
    );
}

#[test]
fn identity_preserves_every_relationship_fact() {
    let plain = graph();
    let mut variants = Vec::new();
    let mut scoped = plain.clone();
    scoped.relations.scope = Some(scope());
    variants.push(scoped.clone());
    scoped.relations.scope.as_mut().unwrap().input = "store".into();
    variants.push(scoped.clone());
    scoped
        .relations
        .scope
        .as_mut()
        .unwrap()
        .values
        .insert("other".into(), ValueType::String);
    variants.push(scoped.clone());
    scoped
        .relations
        .scope
        .as_mut()
        .unwrap()
        .state_outputs
        .insert("counter".into(), "other".into());
    variants.push(scoped.clone());
    scoped.relations.host = Some("host".into());
    variants.push(scoped.clone());
    scoped.relations.key = Some("key".into());
    variants.push(scoped.clone());
    scoped.relations.streams.insert(
        "events".into(),
        serde_json::from_value(json!({"node": "origin", "port": "counter"})).unwrap(),
    );
    variants.push(scoped);
    let mut identities = std::collections::BTreeSet::from([plain.workflow_id().unwrap()]);
    for variant in variants {
        assert_eq!(variant.definition, plain.definition);
        assert!(identities.insert(variant.workflow_id().unwrap()));
    }
}

#[test]
fn identity_preserves_node_operation_operands() {
    let mut original = graph();
    bind_scope(&mut original);
    let mut changed = original.clone();
    changed.definition.nodes[1].operation = Some(NodeOperation::Assign {
        target: StateTarget::Root {
            field: "counter".into(),
        },
    });
    assert_eq!(changed.relations, original.relations);
    assert_ne!(
        changed.workflow_id().unwrap(),
        original.workflow_id().unwrap()
    );
}

#[test]
fn identity_preserves_child_relationships_bindings_definitions_and_orders() {
    let mut parent = graph();
    parent
        .children
        .insert("origin".into(), [("selected".into(), graph())].into());
    let original = parent.workflow_id().unwrap();
    let mut changed = parent.clone();
    child(&mut changed).relations.key = Some("child-key".into());
    assert_ne!(changed.workflow_id().unwrap(), original);
    let mut changed = parent.clone();
    child(&mut changed).definition.nodes[0].kind = "user.changed".into();
    assert_ne!(changed.workflow_id().unwrap(), original);
    let mut changed = parent.clone();
    child(&mut changed).execution_order.reverse();
    assert_ne!(changed.workflow_id().unwrap(), original);
    let mut changed = parent;
    let bindings = changed.children.get_mut(&"origin".into()).unwrap();
    let child = bindings.remove("selected").unwrap();
    bindings.insert("renamed".into(), child);
    assert_ne!(changed.workflow_id().unwrap(), original);
}
