//! Passed Flows remain terminal Task calls even when their owner runs in a Stream.
extern crate mfn_core as _;

#[path = "fixtures/observation_capture.rs"]
mod capture;
#[path = "fixtures/event_collector.rs"]
mod event_collector;
#[path = "fixtures/multi-nodes/src/task_stream.rs"]
mod fixture;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, describe_compiled, instantiate_stream,
};
use mf_runtime::NodeConfig;
use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodePortContract, NodePorts, NodeRegistration, NodeResult, PortSpec, PreparedNode,
    PreparedSubgraph, StreamOptions, TaskNode, ValueType, WorkflowArguments,
};
use mf_telemetry::{
    event::{Event, ScopePathEntry},
    identity::RunId,
    stream::{StreamIdentity, StreamPayload, StreamRecord, StreamTrigger},
};
use serde_json::{Value, json};
use snafu::ensure;
use std::collections::BTreeMap;

const KIND: &str = "test.stream_flow_argument_consumer";
const OWNER: &str = "consume";
const ALIAS: &str = "/local";

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct Config {
    local: PreparedSubgraph,
}

struct Consumer {
    id: String,
    body: PreparedSubgraph,
}

impl NodePortContract for Consumer {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("count", ValueType::Usize, true)],
            outputs: vec![PortSpec::new("total", ValueType::Usize, true)],
        }
    }
}

impl TaskNode for Consumer {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let body = &self.body;
        let outputs = context.run_call(
            &body.scope_key(&self.id, ALIAS),
            WorkflowArguments(Inputs::from([(
                "leaf.count".into(),
                inputs["count"].clone(),
            )])),
            &body.nodes,
            |child| body.execute_in_context(child),
        )?;
        Ok(outputs.into())
    }
}

fn plain(_: Value) -> Result<PreparedNode, NodeBuildError> {
    mf_runtime::NodeInvalidSubgraphSnafu {
        message: "this consumer requires a prepared Flow handle",
    }
    .fail()
}

fn prepared(
    id: &str,
    config: Value,
    arguments: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    let config = Config::from_config(config, arguments)?;
    ensure!(
        config.local.stream_outputs.is_empty(),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "consumer requires exactly its local terminal Flow handle"
        }
    );
    PreparedNode::new(
        Consumer {
            id: id.into(),
            body: config.local,
        },
        NodeMetadata::default(),
    )
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        factory: NodeFactory::Subgraph { flow_parameters: Config::flow_parameters, plain, prepared },
    }
}

fn definition(value: Value) -> WorkflowDefinition {
    serde_json::from_value(value).unwrap()
}

fn terminal_child(task_stream: bool) -> Value {
    json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[if task_stream {
            json!({"id":"leaf", "kind":"fixture.task_stream"})
        } else {
            json!({"id":"leaf", "kind":"builtin.parameters", "config":{
                "parameters":{"count":{"type":"usize", "secret":false}}
            }})
        }],
        "outputs":[{"name":"total", "node":"leaf", "port":if task_stream { "total" } else { "count" }}]
    })
}

fn parent(child: Value) -> Value {
    // No execution configuration: the real Stream source must determine the mode.
    json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[
            {"id":"source", "kind":"fixture.task_stream_feed", "config":{"counts":[3,7]}},
            {"id":OWNER, "kind":KIND, "config":{"local":{"definition":child}},
             "inputs":{"count":{"from":{"node":"source", "port":"count"}}}}
        ],
        "outputs":[{"name":"total", "node":OWNER, "port":"total"}]
    })
}

#[test]
fn stream_consumer_requires_a_matching_prepared_flow_contract() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let registration = registry.get(KIND).unwrap();
    let error = registration.instantiate(json!({})).err().unwrap();
    assert!(
        error
            .to_string()
            .contains("requires a prepared Flow handle")
    );
    let error = registration
        .instantiate_subgraphs(
            OWNER,
            json!({"local":terminal_child(false)}),
            BTreeMap::new(),
        )
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("receiver-selected configuration fields")
    );
}

#[test]
fn stream_messages_call_one_terminal_flow_task_with_bound_scope_and_identity() {
    exercise_stream_parent(false);
}

#[test]
fn terminal_task_stream_child_adapts_without_promoting_the_passed_flow_to_stream() {
    exercise_stream_parent(true);
}

fn exercise_stream_parent(task_stream_child: bool) {
    let registry = NodeRegistry::from_inventory().unwrap();
    let child = terminal_child(task_stream_child);
    let child_plan = compile_definition(&definition(child.clone()), &registry).unwrap();
    assert!(
        child_plan.graph.definition.execution.is_none(),
        "exporting only terminal results must keep the child in Single mode"
    );
    let document = parent(child);
    assert!(document.get("execution").is_none());
    let plan = compile_definition(&definition(document), &registry).unwrap();
    assert_eq!(
        plan.graph.definition.execution,
        Some(mf_runtime::StreamExecution::default())
    );
    assert_eq!(plan.graph.definition.nodes.len(), 2);
    let graph = describe_compiled(&plan).unwrap();
    graph.validate().unwrap();
    assert!(graph.is_streaming());
    assert_eq!(
        graph
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node.kind.as_str()))
            .collect::<Vec<_>>(),
        [("source", "fixture.task_stream_feed"), (OWNER, KIND)],
        "a passed Flow must not create executable wrapper nodes or rewrite its owner's kind"
    );
    assert_eq!(graph.loop_bodies.len(), 1);
    let body_path = vec![ScopePathEntry::binding_key(OWNER, ALIAS)];
    assert_eq!(graph.body_paths_for_node(&[], OWNER), vec![&body_path]);
    assert!(graph.body(&[OWNER.into()]).is_none());
    let body = graph.body(&body_path).unwrap();
    assert_eq!(body.nodes.len(), 1);
    assert_eq!(body.nodes[0].id, "leaf");
    assert_eq!(
        body.nodes[0].kind,
        if task_stream_child {
            "fixture.task_stream"
        } else {
            "builtin.parameters"
        }
    );

    let prepared = instantiate_stream(&plan, &registry).unwrap();
    let owner = prepared
        .plan()
        .nodes()
        .iter()
        .find(|node| node.definition_id.as_str() == OWNER)
        .unwrap();
    assert!(
        owner.node.is_some(),
        "the Flow consumer must remain one TaskNode"
    );
    assert!(owner.metadata.stream_outputs.is_empty());

    let harness = capture::Harness::new(true);
    let observation = plan
        .start_stream_observation(&harness.observer(), RunId::new())
        .unwrap();
    let instance = prepared
        .start_with_options(StreamOptions {
            observation: Some(observation),
            ..Default::default()
        })
        .unwrap();
    let mut deliveries = Vec::new();
    while let Some(output) = instance.recv().unwrap() {
        assert_eq!(output.outputs.len(), 1);
        deliveries.push((output.message, output.outputs["total"].as_u64().unwrap()));
    }
    let summary = instance.join().unwrap();
    assert_eq!(
        deliveries
            .iter()
            .map(|(_, value)| *value)
            .collect::<Vec<_>>(),
        [3, 7]
    );
    assert_eq!(summary.delivered_outputs, 2);
    assert_eq!(
        summary.emitted_messages, 2,
        "terminal-only TaskStream children must not emit extra parent messages"
    );
    assert_eq!(deliveries[0].0.domain, deliveries[1].0.domain);
    assert_ne!(deliveries[0].0.domain, 0);
    assert_ne!(deliveries[0].0, deliveries[1].0);

    let records = harness
        .records()
        .iter()
        .filter(|record| record.scope == mf_telemetry::INSTRUMENTATION_SCOPE)
        .map(|record| StreamRecord::decode(record).unwrap())
        .collect::<Vec<_>>();
    let mut owner_started = Vec::<StreamIdentity>::new();
    let mut owner_finished = Vec::new();
    let mut leaf_started = Vec::new();
    let mut leaf_finished = Vec::new();
    let mut call_started = Vec::new();
    let mut call_finished = Vec::new();
    for record in &records {
        record.validate().unwrap();
        if let StreamPayload::Execution(event) = &record.payload {
            let identity = record
                .identity
                .clone()
                .expect("every execution event needs a StreamIdentity");
            match event {
                Event::NodeStarted { node, .. } if node.id == OWNER => {
                    assert_eq!(node.kind, KIND);
                    assert!(node.path.is_empty());
                    owner_started.push(identity);
                }
                Event::NodeFinished { node, .. } if node.id == OWNER => {
                    owner_finished.push(identity)
                }
                Event::NodeStarted { node, .. } if node.id == "leaf" => {
                    graph.validate_scope_path(&node.path).unwrap();
                    leaf_started.push((identity, node.path.clone()));
                }
                Event::NodeFinished { node, .. } if node.id == "leaf" => {
                    leaf_finished.push((identity, node.path.clone()))
                }
                Event::CallStarted { path, .. } => call_started.push((identity, path.clone())),
                Event::CallFinished { path, .. } => call_finished.push((identity, path.clone())),
                _ => {}
            }
        }
    }
    assert_eq!(owner_started.len(), 2);
    assert_eq!(owner_finished, owner_started);
    assert_eq!(leaf_started.len(), 2);
    assert_eq!(leaf_finished, leaf_started);
    assert_eq!(call_started.len(), 2);
    assert_eq!(call_finished, call_started);
    for (index, (identity, path)) in leaf_started.iter().enumerate() {
        let owner_identity = &owner_started[index];
        assert_eq!(
            &call_started[index],
            &(owner_identity.clone(), path.clone())
        );
        assert_eq!(
            identity.parent,
            Some(owner_identity.invocation),
            "the child's distinct invocation must link to its owning Task"
        );
        assert_ne!(identity.invocation, owner_identity.invocation);
        assert_eq!(identity.message, owner_identity.message);
        assert_eq!(identity.trigger, StreamTrigger::Message);
        let message = identity.message.unwrap();
        assert_eq!(message.domain, deliveries[index].0.domain);
        assert_eq!(message.sequence, deliveries[index].0.sequence);
        assert_eq!(path.len(), 1);
        assert!(matches!(&path[0], ScopePathEntry::Call { call_id }
            if call_id == &ScopePathEntry::binding_key(OWNER, ALIAS)));
        assert_eq!(
            path[0].node_id().to_owned(),
            body_path[0],
            "dynamic invocations must resolve to the same static bound body"
        );
    }
    assert_ne!(owner_started[0].invocation, owner_started[1].invocation);
    assert_ne!(owner_started[0].message, owner_started[1].message);
    assert_ne!(leaf_started[0].0.invocation, leaf_started[1].0.invocation);
    assert!(!harness.spans.get_finished_spans().unwrap().is_empty());
}

#[test]
fn inferred_stream_and_event_children_are_rejected_as_passed_terminal_flows() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for child in [
        json!({
            "version":"2026-10-28", "dependencies":{},
            "nodes":[{"id":"leaf", "kind":"fixture.task_stream_feed", "config":{"counts":[3,7]}}],
            "outputs":[{"name":"total", "node":"leaf", "port":"count"}]
        }),
        json!({
            "version":"2026-10-28", "dependencies":{},
            "nodes":[
                {"id":"item", "kind":"builtin.constant", "config":{"value":3}},
                {"id":"leaf", "kind":"test.accumulate", "config":{"max_items":1},
                 "inputs":{"item":{"from":{"node":"item", "port":"value"}}}}
            ],
            "outputs":[{"name":"total", "node":"leaf", "port":"items"}]
        }),
    ] {
        assert!(child.get("execution").is_none());
        let child_plan = compile_definition(&definition(child.clone()), &registry).unwrap();
        assert!(
            child_plan.graph.definition.execution.is_some(),
            "the child itself must infer Stream mode"
        );
        let error = compile_definition(&definition(parent(child)), &registry).unwrap_err();
        // Inferred children are rejected during synchronous body preparation, rather than
        // the earlier explicit-execution guard. Retain the typed body-preparation cause.
        assert!(
            matches!(&error, mf_compiler::WorkflowCompileError::ChildGraph { .. }),
            "{error}"
        );
        assert!(
            error.to_string().contains("requires streaming execution"),
            "{error}"
        );
    }
}
