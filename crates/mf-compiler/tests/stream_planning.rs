#[path = "fixtures/controlled_source.rs"]
mod controlled;
extern crate mfn_core as _;
use mf_compiler::{
    CompiledWorkflow, Inputs, NodeExecutionError, NodeRegistration, NodeRegistry, PortSpec,
    ValueType, WorkflowDefinition, compile_definition, instantiate_stream, plan_definition,
};
use mf_runtime::{EventContext, EventEffects, EventNode, NodeEvent};
use serde_json::{Value, json};

struct Collector;

impl EventNode for Collector {
    fn on_event(
        &mut self,
        _: NodeEvent,
        _: &EventContext<'_>,
    ) -> Result<EventEffects, NodeExecutionError> {
        panic!("planning delivered an event")
    }
}
inventory::submit! {
    NodeRegistration {
        kind: "test.collect",
        factory: mf_runtime::NodeFactory::Plain(|_| {
            Ok(mf_runtime::PreparedNode::event(
                Collector,
                mf_runtime::NodePorts {
                        inputs: vec![PortSpec::new("item", ValueType::Any, true)],
                        outputs: vec![PortSpec::new("items", ValueType::Array, true)],
                    },
            ))
        }),
    }
}

struct Join;
impl mf_runtime::TaskNode for Join {
    fn execute(
        &self,
        _: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        panic!("planning executed a node")
    }
}
inventory::submit! {
    NodeRegistration {
        kind: "test.join",
        factory: mf_runtime::NodeFactory::Plain(|_| {
            Ok(mf_runtime::PreparedNode::new(
                Join,
                mf_runtime::NodePorts {
                        inputs: vec![
                            PortSpec::new("left", ValueType::Any, true),
                            PortSpec::new("right", ValueType::Any, true),
                        ],
                        outputs: vec![PortSpec::new("value", ValueType::Any, true)],
                    },
            ))
        }),
    }
}

fn edge(source: &str, port: &str, target: &str, input: &str) -> Value {
    json!({"from_node":source, "from_output":port, "to_node":target, "to_input":input})
}
fn graph(mut nodes: Value, edges: Vec<Value>) -> Value {
    nodes
        .as_array_mut()
        .unwrap()
        .push(controlled::source(json!("int")));
    json!({"version":"2026-10-03", "execution":{"mode":"stream"},
        "dependencies":{}, "nodes":nodes, "edges":edges})
}
fn prepare(value: Value) -> Result<mf_runtime::PreparedStream, mf_compiler::WorkflowCompileError> {
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry)?;
    let plan = CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap();
    instantiate_stream(&plan, &registry)
}
fn collect_graph() -> Value {
    graph(
        json!([{"id":"collect", "kind":"test.collect"}, {"id":"copy", "kind":"builtin.identity"}]),
        vec![
            edge("feed", "item", "collect", "item"),
            edge("collect", "items", "copy", "input"),
        ],
    )
}

#[test]
fn prepares_typed_input_and_new_message_domains_without_execution() {
    let mut value = collect_graph();
    value["outputs"] = json!([{"name":"result", "node":"copy", "port":"value"}]);
    let prepared = prepare(value.clone()).unwrap();
    assert_eq!(prepared.plan().nodes()[0].definition_id.as_str(), "feed");
    assert_eq!(
        prepared.plan().nodes()[0].metadata.ports.outputs[0].value_type,
        ValueType::Int64
    );
    assert_eq!(
        prepared.plan().nodes()[1].metadata.ports.outputs[0].value_type,
        ValueType::Array
    );
    assert_eq!(prepared.plan().domains().len(), 3);
    assert_eq!(prepared.plan().domains()[0].source, None);
    assert_eq!(prepared.plan().domains()[0].steps, [0]);
    assert_eq!(prepared.plan().domains()[1].source, Some(0));
    assert_eq!(prepared.plan().domains()[1].steps, [1]);
    assert_eq!(prepared.plan().domains()[2].source, Some(1));
    assert_eq!(prepared.plan().domains()[2].steps, [2]);
    assert_eq!(
        (0..3)
            .map(|node| prepared.plan().output_domain(node))
            .collect::<Vec<_>>(),
        [1, 2, 2]
    );
    assert_eq!(prepared.plan().selected_domain(), Some(2));
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let plan = plan_definition(&definition).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    for (version, workers, expected) in [
        ("2026-09-29", 4, "requires workflow schema 2026-10-03"),
        ("2026-10-03", 0, "stream limits must be positive"),
    ] {
        let mut value = serde_json::to_value(&plan).unwrap();
        value["definition"]["version"] = json!(version);
        value["definition"]["execution"]["limits"]["workers"] = json!(workers);
        let definition = serde_json::from_value(value["definition"].clone()).unwrap();
        let error = compile_definition(&definition, &registry)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
        let error = CompiledWorkflow::from_json(&value.to_string())
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
    }

    assert_eq!(plan.definition.nodes.len(), 3);
    assert_eq!(
        plan.execution_order
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["feed", "collect", "copy"]
    );
    assert_eq!(
        prepare(graph(json!([]), vec![]))
            .unwrap()
            .plan()
            .domains()
            .len(),
        2
    );
}

#[test]
fn validates_fanout_rejoins_chained_batches_and_selected_domains() {
    let value = graph(
        json!([
            {"id":"left", "kind":"builtin.identity"}, {"id":"right", "kind":"builtin.identity"},
            {"id":"join", "kind":"test.join"}
        ]),
        vec![
            edge("feed", "item", "left", "input"),
            edge("feed", "item", "right", "input"),
            edge("left", "value", "join", "left"),
            edge("right", "value", "join", "right"),
        ],
    );
    let prepared = prepare(value).unwrap();
    assert_eq!(prepared.plan().domains().len(), 2);
    assert!(
        (0..prepared.plan().nodes().len()).all(|node| prepared.plan().output_domain(node) == 1)
    );

    let chained = graph(
        json!([{"id":"first", "kind":"test.collect"}, {"id":"second", "kind":"test.collect"}]),
        vec![
            edge("feed", "item", "first", "item"),
            edge("first", "items", "second", "item"),
        ],
    );
    let prepared = prepare(chained).unwrap();
    assert_eq!(prepared.plan().domains().len(), 4);
    assert_eq!(
        prepared
            .plan()
            .domains()
            .iter()
            .map(|domain| (domain.source, domain.steps.as_slice()))
            .collect::<Vec<_>>(),
        [
            (None, [0].as_slice()),
            (Some(0), [1].as_slice()),
            (Some(1), [2].as_slice()),
            (Some(2), [].as_slice())
        ]
    );
    assert_eq!(
        (0..3)
            .map(|node| prepared.plan().output_domain(node))
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );

    let mut independent = graph(
        json!([
            {"id":"first", "kind":"test.collect"}, {"id":"second", "kind":"test.collect"}, {"id":"join", "kind":"test.join"}
        ]),
        vec![
            edge("feed", "item", "first", "item"),
            edge("feed", "item", "second", "item"),
            edge("first", "items", "join", "left"),
            edge("second", "items", "join", "right"),
        ],
    );
    let error = prepare(independent.clone()).unwrap_err().to_string();
    assert!(
        error.contains("message domain")
            && error.contains("first.items")
            && error.contains("second.items"),
        "{error}"
    );
    independent["edges"][3] = edge("feed", "item", "join", "right");
    assert!(
        prepare(independent)
            .unwrap_err()
            .to_string()
            .contains("message domain")
    );

    let mut selected = collect_graph();
    selected["outputs"] = json!([{"name":"item", "node":"feed", "port":"item"}, {"name":"batch", "node":"collect", "port":"items"}]);
    assert!(
        prepare(selected)
            .unwrap_err()
            .to_string()
            .contains("selected output")
    );
}

#[test]
fn controls_and_context_references_obey_the_message_boundary() {
    let mut value = collect_graph();
    value["control_edges"] = json!([{"from_node":"feed", "from_output":"item", "to_node":"copy"}]);
    assert!(
        prepare(value)
            .unwrap_err()
            .to_string()
            .contains("message domain")
    );
    let mut value = collect_graph();
    value["nodes"].as_array_mut().unwrap().push(json!({"id":"route", "kind":"builtin.if_else", "config":{"branches":[{
        "id":"yes", "condition":{"source":{"output":"feed.item", "path":""}, "operator":"eq", "value":1}
    }]}}));
    value["control_edges"] =
        json!([{"from_node":"copy", "from_output":"value", "to_node":"route"}]);
    let error = prepare(value.clone()).unwrap_err().to_string();
    assert!(
        error.contains("context reference") && error.contains("feed.item"),
        "{error}"
    );
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["id"] == "route")
        .unwrap()["config"]["branches"][0]["condition"] =
        json!({"source":{"output":"copy.value", "path":"/0"}, "operator":"eq", "value":1});
    value["outputs"] = json!([{"name":"selected", "node":"route", "port":"yes", "optional":true}]);
    assert_eq!(prepare(value).unwrap().plan().selected_domain(), Some(2));
}

#[test]
fn sources_are_explicit_and_independent_initial_tasks_need_no_trigger() {
    let mut value = graph(
        json!([{"id":"constant", "kind":"builtin.constant", "config":{"value":42}}]),
        vec![],
    );
    let prepared = prepare(value.clone()).unwrap();
    assert!(
        prepared
            .plan()
            .input_schema()
            .inputs
            .contains_key("constant")
    );
    value["control_edges"] =
        json!([{"from_node":"feed", "from_output":"item", "to_node":"constant"}]);
    let prepared = prepare(value).unwrap();
    assert!(
        !prepared
            .plan()
            .input_schema()
            .inputs
            .contains_key("constant")
    );
    let mut value = collect_graph();
    value["edges"][0]["from_output"] = json!("unknown");
    assert!(prepare(value).unwrap_err().to_string().contains("unknown"));
}

#[test]
fn rejects_event_nodes_in_single_runs_and_synchronous_bodies() {
    let mut value = graph(
        json!([
            {"id":"source", "kind":"builtin.constant", "config":{"value":1}},
            {"id":"collect", "kind":"test.collect"}
        ]),
        vec![edge("source", "value", "collect", "item")],
    );
    value.as_object_mut().unwrap().remove("execution");
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    assert!(
        compile_definition(&definition, &NodeRegistry::from_inventory().unwrap())
            .unwrap_err()
            .to_string()
            .contains("requires streaming")
    );

    let iteration = json!({"id":"iterate", "kind":"builtin.iteration", "config":{"body":{
        "nodes":[{"id":"collect", "kind":"test.collect"}],
        "edges":[edge("%iteration", "item", "collect", "item")],
        "result":{"node":"collect", "port":"items"}
    }}});
    let mut value = graph(
        json!([iteration]),
        vec![edge("feed", "item", "iterate", "items")],
    );
    value["nodes"].as_array_mut().unwrap().last_mut().unwrap()["config"]["item_type"] =
        json!("array");
    let error = prepare(value).unwrap_err().to_string();
    assert!(
        error.contains("iterate") && error.contains("synchronous scope"),
        "{error}"
    );

    let repeat = json!({"id":"repeat", "kind":"workflow.loop", "loop":{
        "max_iterations":1, "variables":[{"name":"x", "type":"int"}], "body":{
            "nodes":[{"id":"collect", "kind":"test.collect"}], "edges":[edge("%loop", "x", "collect", "item")]
        }
    }});
    let error = prepare(graph(
        json!([repeat]),
        vec![edge("feed", "item", "repeat", "x")],
    ))
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("repeat") && error.contains("synchronous scope"),
        "{error}"
    );
}
