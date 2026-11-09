use mf_runtime::{
    CompiledGraph, GraphRelations, ScopeInstruction, ScopePathEntry, ScopeRelations, ValueType,
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
        .get_mut("/selected")
        .unwrap()
}

fn scope() -> ScopeRelations {
    ScopeRelations {
        input: "origin".into(),
        values: [("counter".into(), ValueType::String)].into(),
        state_outputs: [("counter".into(), "result".into())].into(),
        writes: [("store".into(), "counter".into())].into(),
        exits: ["stop".into()].into(),
    }
}

#[test]
fn instruction_roles_follow_explicit_relations_not_source_names_or_kinds() {
    let mut graph = graph();
    graph.relations.scope = Some(scope());
    for kind in ["user.noop", "renamed.provider", "builtin.iteration_input"] {
        for node in &mut graph.definition.nodes {
            node.kind = kind.into();
        }
        assert_eq!(
            graph.relations.instruction(&"origin".into()),
            Some(ScopeInstruction::Input)
        );
        assert_eq!(
            graph.relations.instruction(&"store".into()),
            Some(ScopeInstruction::Write("counter"))
        );
        assert_eq!(
            graph.relations.instruction(&"stop".into()),
            Some(ScopeInstruction::Exit)
        );
        assert_eq!(graph.relations.instruction(&"%loop".into()), None);
        assert_eq!(graph.relations.instruction(&"%iteration".into()), None);
    }
    let scope = graph.relations.scope.as_mut().unwrap();
    scope.input = "another-source".into();
    assert_eq!(graph.relations.instruction(&"origin".into()), None);
    assert_eq!(
        graph.relations.instruction(&"another-source".into()),
        Some(ScopeInstruction::Input)
    );
    graph.relations.scope = None;
    assert_eq!(graph.relations.instruction(&"store".into()), None);
    assert_eq!(graph.relations.instruction(&"stop".into()), None);
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
    assert!(!scope.writes.is_empty());
}

#[test]
fn child_scope_keys_use_explicit_override_or_generic_binding_for_every_kind() {
    let mut child = graph();
    let mut owner = child.definition.nodes[0].clone();
    owner.id = "owner/with~punctuation".into();
    for kind in [
        "workflow.call",
        "workflow.loop",
        "builtin.iteration",
        "user.renamed",
    ] {
        owner.kind = kind.into();
        for binding in ["/body", "/other"] {
            assert_eq!(
                child.child_scope_key(&owner, binding),
                ScopePathEntry::binding_key(owner.id.as_str(), binding)
            );
        }
    }
    child.relations.key = Some("explicit-edge-identity".into());
    assert_eq!(
        child.child_scope_key(&owner, "/body"),
        "explicit-edge-identity"
    );
    assert_eq!(
        child.child_scope_key(&owner, "/other"),
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
    assert!(serde_json::from_value::<GraphRelations>(json!({"unknown": true})).is_err());
    let encoded = serde_json::to_value(scope()).unwrap();
    for field in ["input", "values", "state_outputs", "writes", "exits"] {
        let mut missing = encoded.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<ScopeRelations>(missing).is_err());
    }
    let mut unknown = encoded;
    unknown["unknown"] = json!(true);
    assert!(serde_json::from_value::<ScopeRelations>(unknown).is_err());
}

#[test]
fn all_concrete_relationships_round_trip() {
    let mut graph = graph();
    graph.relations.scope = Some(scope());
    graph.relations.host = Some("parent-host".into());
    graph.relations.key = Some("edge-key".into());
    graph.relations.streams.insert(
        "events".into(),
        serde_json::from_value(json!({"node": "origin", "port": "counter"})).unwrap(),
    );
    assert_eq!(
        serde_json::from_slice::<CompiledGraph>(&serde_json::to_vec(&graph).unwrap()).unwrap(),
        graph
    );
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
    graph.relations.scope = Some(scope());
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
    let scope = scoped.relations.scope.as_mut().unwrap();
    scope.input = "store".into();
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
    scoped
        .relations
        .scope
        .as_mut()
        .unwrap()
        .writes
        .insert("stop".into(), "counter".into());
    variants.push(scoped.clone());
    scoped
        .relations
        .scope
        .as_mut()
        .unwrap()
        .exits
        .insert("store".into());
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
    let mut identities = std::collections::BTreeSet::new();
    identities.insert(plain.workflow_id().unwrap());
    for variant in variants {
        assert_eq!(variant.definition, plain.definition);
        assert!(identities.insert(variant.workflow_id().unwrap()));
    }
}

#[test]
fn identity_preserves_child_relationships_bindings_definitions_and_orders() {
    let mut parent = graph();
    parent
        .children
        .insert("origin".into(), [("/selected".into(), graph())].into());
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
    let child = bindings.remove("/selected").unwrap();
    bindings.insert("/renamed".into(), child);
    assert_ne!(changed.workflow_id().unwrap(), original);
}
