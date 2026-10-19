extern crate mfn_code as _;
extern crate mfn_core as _;
#[path = "fixtures/controlled_source.rs"]
mod controlled;
#[path = "fixtures/multi-nodes/src/context_fixture.rs"]
mod fixture;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use serde_json::{Value, json};
use std::path::Path;

fn task(id: &str, trace: &Path) -> Value {
    json!({"id":id,"kind":"fixture.context","config":{"name":id,"trace":trace,"ports":[]}})
}
fn definition(nodes: Vec<Value>, edges: Value) -> WorkflowDefinition {
    serde_json::from_value(
        json!({"version":"2026-10-19","dependencies":{},"nodes":nodes,"edges":edges}),
    )
    .unwrap()
}
fn trace(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}
fn execute(
    definition: &WorkflowDefinition,
) -> Result<mf_runtime::FlowOutputs, mf_compiler::WorkflowExecutionError> {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(definition, &registry)?;
    mf_compiler::execute_compiled(&plan, &registry, None)
}

#[test]
fn zero_output_success_orders_forks_and_joins_without_observation() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("trace");
    let graph = definition(
        ["a_join", "b_left", "c_right", "z_start"]
            .map(|id| task(id, &path))
            .to_vec(),
        json!([
            {"from_node":"z_start","to_node":"b_left"},{"from_node":"z_start","to_node":"c_right"},
            {"from_node":"b_left","to_node":"a_join"},{"from_node":"c_right","to_node":"a_join"}
        ]),
    );
    execute(&graph).unwrap();
    let calls = trace(&path);
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[0], "z_start");
    assert_eq!(calls[3], "a_join");
}

#[test]
fn success_is_distinct_from_an_optional_output_skip() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("trace");
    let mut source = task("source", &path);
    source["config"]["ports"] = json!(["optional"]);
    source["config"]["skipped"] = json!(["optional"]);
    let graph = definition(
        vec![
            source,
            task("after_success", &path),
            task("after_output", &path),
        ],
        json!([
            {"from_node":"source","to_node":"after_success"},
            {"from_node":"source","from_output":"optional","to_node":"after_output"}
        ]),
    );
    execute(&graph).unwrap();
    assert_eq!(trace(&path), ["source", "after_success"]);
}

#[test]
fn failures_and_invalid_publications_never_activate_success_consumers() {
    for failure in ["execute", "publish"] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("trace");
        let mut source = task("source", &path);
        if failure == "execute" {
            source["config"]["fail"] = json!(true);
        } else {
            source["config"]["outputs"] = json!({"undeclared":42});
        }
        let graph = definition(
            vec![source, task("target", &path)],
            json!([{"from_node":"source","to_node":"target"}]),
        );
        assert!(execute(&graph).is_err());
        assert_eq!(trace(&path), ["source"]);
    }
}

fn router(source: &str) -> Value {
    json!({"id":"route","kind":"builtin.if_else","config":{"branches":[{
        "id":"positive","condition":{"source":{"output":source,"path":""},"operator":"gt","value":0}
    }]}})
}

#[test]
fn skipped_sources_cannot_reactivate_a_branch() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("trace");
    let graph = definition(
        vec![
            json!({"id":"value","kind":"builtin.constant","config":{"value":-1}}),
            router("value.value"),
            task("source", &path),
            task("target", &path),
        ],
        json!([
            {"from_node":"value","to_node":"route"},
            {"from_node":"route","from_output":"positive","to_node":"source"},
            {"from_node":"source","to_node":"target"}
        ]),
    );
    execute(&graph).unwrap();
    assert!(trace(&path).is_empty());
}

#[test]
fn router_success_does_not_prove_branches_but_gated_task_success_does() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("trace");
    let nodes = vec![
        json!({"id":"value","kind":"builtin.constant","config":{"value":1}}),
        router("value.value"),
        task("left_gate", &path),
        task("right_gate", &path),
        json!({"id":"left","kind":"builtin.constant","config":{"value":1}}),
        json!({"id":"right","kind":"builtin.constant","config":{"value":2}}),
        json!({"id":"join","kind":"builtin.identity","inputs":{"input":{"exclusive":[{"node":"left","port":"value"},{"node":"right","port":"value"}]}}}),
    ];
    let gates = json!([
        {"from_node":"value","to_node":"route"},
        {"from_node":"route","from_output":"positive","to_node":"left_gate"},
        {"from_node":"route","from_output":"else","to_node":"right_gate"},
        {"from_node":"left_gate","to_node":"left"},{"from_node":"right_gate","to_node":"right"}
    ]);
    let graph = definition(nodes.clone(), gates);
    execute(&graph).unwrap();
    let graph = definition(
        nodes,
        json!([
            {"from_node":"value","to_node":"route"},
            {"from_node":"route","to_node":"left"},{"from_node":"route","to_node":"right"}
        ]),
    );
    assert!(
        compile_definition(&graph, &NodeRegistry::from_inventory().unwrap())
            .unwrap_err()
            .to_string()
            .contains("prove")
    );
}

#[test]
fn stream_messages_have_independent_success_outcomes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("trace");
    let mut graph = definition(
        vec![
            controlled::source(json!("int")),
            router("feed.item"),
            task("source", &path),
            task("target", &path),
        ],
        json!([
            {"from_node":"feed","from_output":"item","to_node":"route"},
            {"from_node":"route","from_output":"positive","to_node":"source"},
            {"from_node":"source","to_node":"target"}
        ]),
    );
    graph.execution = Some(serde_json::from_value(json!({"mode":"stream"})).unwrap());
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&graph, &registry).unwrap();
    let run = controlled::SourceRun::start(
        mf_compiler::instantiate_stream(&plan, &registry).unwrap(),
        "feed",
        Default::default(),
    )
    .unwrap();
    run.source.send(json!(1)).unwrap();
    run.source.send(json!(-1)).unwrap();
    run.source.close();
    while run.recv().unwrap().is_some() {}
    run.join().unwrap();
    assert_eq!(trace(&path), ["source", "target"]);
}

#[test]
fn programmatic_builders_and_stream_domains_enforce_success_boundaries() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("trace");
    let mut graph = definition(
        vec![controlled::source(json!("int")), task("target", &path)],
        json!([{"from_node":"feed","to_node":"target"}]),
    );
    graph.execution = Some(serde_json::from_value(json!({"mode":"stream"})).unwrap());
    let registry = NodeRegistry::from_inventory().unwrap();
    assert!(compile_definition(&graph, &registry).is_err());
    let nodes = mf_compiler::resolve_nodes(&graph, &registry).unwrap();
    let builder = mf_compiler::FlowBuilder::prepare(
        nodes,
        vec![],
        vec!["feed".into(), "target".into()],
        vec![],
    )
    .unwrap();
    assert!(builder.with_order_edges(graph.order_edges.clone()).is_err());
    let graph = definition(
        vec![task("a", &path), task("b", &path)],
        json!([{"from_node":"a","to_node":"b"}]),
    );
    let plan = compile_definition(&graph, &registry).unwrap();
    assert!(instantiate_compiled(&plan, &registry).is_ok());
}

#[test]
fn body_invocations_keep_success_states_local_and_containers_are_rejected() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for source in [
        include_str!("../../../examples/loop.json"),
        include_str!("../../../examples/iteration.json"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("trace");
        let mut graph: WorkflowDefinition = serde_json::from_str(source).unwrap();
        graph.version = mf_compiler::WorkflowDefinitionVersion::V2026_10_19;
        let container = graph.nodes[1].id.clone();
        if let Some(loop_definition) = &mut graph.nodes[1].loop_definition {
            loop_definition.body.nodes.extend([
                serde_json::from_value(task("a", &path)).unwrap(),
                serde_json::from_value(task("b", &path)).unwrap(),
            ]);
            loop_definition
                .body
                .order_edges
                .push(mf_compiler::OrderEdgeDefinition {
                    from_node: "a".into(),
                    to_node: "b".into(),
                });
        } else {
            let body = &mut graph.nodes[1].config["body"];
            body["nodes"]
                .as_array_mut()
                .unwrap()
                .extend([task("a", &path), task("b", &path)]);
            body["order_edges"] = json!([{"from_node":"a","to_node":"b"}]);
        }
        execute(&graph).unwrap();
        assert_eq!(trace(&path), ["a", "b", "a", "b", "a", "b"]);
        graph
            .nodes
            .push(serde_json::from_value(task("after", &path)).unwrap());
        graph.order_edges.push(mf_compiler::OrderEdgeDefinition {
            from_node: container,
            to_node: "after".into(),
        });
        assert!(compile_definition(&graph, &registry).is_err());
    }
}

#[test]
fn success_edges_cannot_join_startup_and_message_domains() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("trace");
    let mut target = task("target", &path);
    target["config"]["inputs"] = json!(["item"]);
    target["inputs"] = json!({"item":{"from":{"node":"feed","port":"item"}}});
    let mut graph = definition(
        vec![
            controlled::source(json!("int")),
            task("initial", &path),
            target,
        ],
        json!([{"from_node":"initial","to_node":"target"}]),
    );
    graph.execution = Some(serde_json::from_value(json!({"mode":"stream"})).unwrap());
    assert!(
        compile_definition(&graph, &NodeRegistry::from_inventory().unwrap())
            .unwrap_err()
            .to_string()
            .contains("message domain")
    );
}
