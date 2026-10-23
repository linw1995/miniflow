#[path = "fixtures/observation_capture.rs"]
mod capture;
#[path = "fixtures/multi-nodes/src/task_stream.rs"]
mod fixture;

extern crate mfn_core as _;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled, instantiate_stream,
};
use mf_runtime::{StreamOptions, WorkflowArguments};
use serde_json::json;
use std::sync::{Arc, Mutex};

fn root_export() -> serde_json::Value {
    json!({
        "version": "2026-10-23", "dependencies": {},
        "nodes": [{"id": "group.count", "kind": "fixture.task_stream"}],
        "outputs": [{"name": "total", "node": "group.count", "port": "total"}],
        "stream_outputs": [{"name": "increments", "path": ["group.count"], "port": "delta"}]
    })
}

fn run_live(
    value: serde_json::Value,
    arguments: &str,
) -> (mf_runtime::FlowOutputs, Vec<mf_runtime::LiveOutputEvent>) {
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let mut context = mf_runtime::ExecutionContext::default();
    context.set_live_publisher(mf_runtime::LivePublisher::new(move |event, _| {
        captured.lock().unwrap().push(event);
        Ok(())
    }));
    context.set_workflow_arguments(WorkflowArguments::from_json(arguments.as_bytes()).unwrap());
    let result = flow.execute_in_context(&mut context).unwrap();
    let events = events.lock().unwrap().clone();
    (result, events)
}

#[test]
fn external_combined_provider_retains_terminal_execution_in_both_flow_modes() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for streaming in [false, true] {
        let mut value = json!({
            "version": "2026-10-19",
            "dependencies": {},
            "nodes": [{"id": "count", "kind": "fixture.task_stream"}],
            "outputs": [{"name": "total", "node": "count", "port": "total"}]
        });
        if streaming {
            value["execution"] = json!({"mode": "stream"});
        }
        let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let arguments = WorkflowArguments::from_json(br#"{"count.count":100}"#).unwrap();
        let output = if streaming {
            let instance = instantiate_stream(&plan, &registry)
                .unwrap()
                .start_with_options(StreamOptions {
                    arguments,
                    ..Default::default()
                })
                .unwrap();
            let output = instance.recv().unwrap().unwrap().outputs;
            assert!(instance.recv().unwrap().is_none());
            instance.join().unwrap();
            output
        } else {
            instantiate_compiled(&plan, &registry)
                .unwrap()
                .execute_with_inputs(arguments)
                .unwrap()
        };
        assert_eq!(output["total"].as_u64(), Some(100));
    }
}

#[test]
fn finite_exports_preserve_dotted_ids_and_close_empty_invocations() {
    for count in [0, 3] {
        let (output, events) = run_live(
            root_export(),
            &format!(r#"{{"group.count.count":{count}}}"#),
        );
        assert_eq!(output["total"].as_u64(), Some(count));
        assert_eq!(events.len(), count as usize + 2);
        assert!(matches!(events[0].event, mf_runtime::LiveOutputKind::Open));
        assert!(matches!(
            events.last().unwrap().event,
            mf_runtime::LiveOutputKind::Close { completed: true }
        ));
        for event in &events {
            assert_eq!(event.export, "increments");
            assert_eq!(
                event.source,
                vec![mf_runtime::DefinitionId::from("group.count")]
            );
            assert_eq!(event.invocation.node, "group.count");
            assert_eq!(event.invocation.id, events[0].invocation.id);
        }
    }
}

#[test]
fn combined_terminal_success_prerequisites_retain_ordinary_task_behavior() {
    let mut document = root_export();
    document["nodes"].as_array_mut().unwrap().push(json!({
        "id":"after", "kind":"builtin.identity", "inputs":{"input":{"from":{"node":"group.count","port":"total"}}}
    }));
    document["edges"] = json!([{"from_node":"group.count","to_node":"after"}]);
    document["outputs"] = json!([{"name":"total","node":"after","port":"value"}]);
    let (output, events) = run_live(document, r#"{"group.count.count":3}"#);
    assert_eq!(output["total"].as_u64(), Some(3));
    assert_eq!(events.len(), 5);
}

#[test]
fn export_definitions_round_trip_and_invalid_sources_fail_before_execution() {
    let definition: WorkflowDefinition = serde_json::from_value(root_export()).unwrap();
    assert_eq!(
        serde_json::from_value::<WorkflowDefinition>(serde_json::to_value(&definition).unwrap())
            .unwrap(),
        definition
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    for (field, value, expected) in [
        ("path", json!(["missing"]), "source path"),
        ("port", json!("total"), "not a stream output"),
        ("name", json!(""), "nonempty"),
    ] {
        let mut document = root_export();
        document["stream_outputs"][0][field] = value;
        let definition = serde_json::from_value(document).unwrap();
        assert!(
            compile_definition(&definition, &registry)
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
    }
    let mut duplicate = root_export();
    duplicate["stream_outputs"] = json!([
        duplicate["stream_outputs"][0],
        duplicate["stream_outputs"][0]
    ]);
    assert!(compile_definition(&serde_json::from_value(duplicate).unwrap(), &registry).is_err());
    let mut legacy = root_export();
    legacy["version"] = json!("2026-10-19");
    assert!(serde_json::from_value::<WorkflowDefinition>(legacy).is_err());
}

#[test]
fn scoped_exports_preserve_loop_and_parallel_iteration_invocations() {
    for kind in ["loop", "sequential", "parallel"] {
        let (subgraph, output, expected) = if kind == "loop" {
            (
                json!({
                    "id":"repeat", "kind":"workflow.loop", "inputs":{"count":{"from":{"node":"seed","port":"value"}}},
                    "loop":{"max_iterations":2,"variables":[{"name":"count","type":"usize"}],"body":{
                        "nodes":[{"id":"count","kind":"fixture.task_stream","inputs":{"count":{"from":{"node":"%loop","port":"count"}}}}]
                    }}
                }),
                "count",
                json!(2),
            )
        } else {
            (
                json!({
                    "id":"repeat", "kind":"builtin.iteration", "inputs":{"items":{"from":{"node":"seed","port":"value"}}},
                    "config":{"mode":kind,"body":{
                        "nodes":[{"id":"count","kind":"fixture.task_stream","inputs":{"count":{"from":{"node":"%iteration","port":"item"}}}}],
                        "result":{"node":"count","port":"total"}
                    }}
                }),
                "results",
                json!([1, 2]),
            )
        };
        let document = json!({
            "version":"2026-10-23", "dependencies":{},
            "nodes":[{"id":"seed","kind":"builtin.constant","config":{"value":if kind == "loop" {json!(2)} else {json!([1,2])}}},subgraph],
            "outputs":[{"name":"result","node":"repeat","port":output}],
            "stream_outputs":[{"name":"increments","path":["repeat","count"],"port":"delta"}]
        });
        let (result, events) = run_live(document, "{}");
        assert_eq!(serde_json::to_value(&result["result"]).unwrap(), expected);
        let opens = events
            .iter()
            .filter(|event| matches!(event.event, mf_runtime::LiveOutputKind::Open))
            .collect::<Vec<_>>();
        let closes = events
            .iter()
            .filter(|event| {
                matches!(
                    event.event,
                    mf_runtime::LiveOutputKind::Close { completed: true }
                )
            })
            .count();
        assert_eq!(opens.len(), 2);
        assert_eq!(closes, 2);
        assert_ne!(opens[0].invocation.id, opens[1].invocation.id);
        for event in opens {
            assert_eq!(event.source, vec!["repeat".into(), "count".into()]);
            assert_eq!(event.invocation.path.len(), 1);
            assert_eq!(event.invocation.path[0].loop_id, "repeat");
        }
    }
}

#[test]
fn planner_separates_terminal_and_stream_domains_and_rejects_implicit_joins() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut document = root_export();
    document["execution"] = json!({"mode":"stream"});
    document["nodes"].as_array_mut().unwrap().push(json!({
        "id":"consume","kind":"builtin.identity", "inputs":{"input":{"from":{"node":"group.count","port":"delta"}}}
    }));
    let definition = serde_json::from_value(document.clone()).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let prepared = instantiate_stream(&plan, &registry).unwrap();
    let source = prepared
        .plan()
        .nodes()
        .iter()
        .position(|node| node.definition_id.as_str() == "group.count")
        .unwrap();
    assert_eq!(prepared.plan().output_domain(source), 0);
    assert_eq!(prepared.plan().stream_output_domain(source), Some(1));
    document.as_object_mut().unwrap().remove("execution");
    assert!(
        compile_definition(
            &serde_json::from_value(document.clone()).unwrap(),
            &registry
        )
        .unwrap_err()
        .to_string()
        .contains("requires streaming execution")
    );
    document["execution"] = json!({"mode":"stream"});
    document["nodes"][1] = json!({"id":"consume","kind":"fixture.task_stream_join", "inputs":{
        "delta":{"from":{"node":"group.count","port":"delta"}},
        "total":{"from":{"node":"group.count","port":"total"}}
    }});
    let error = compile_definition(&serde_json::from_value(document).unwrap(), &registry)
        .unwrap_err()
        .to_string();
    assert!(error.contains("message domain"), "{error}");
}

#[test]
fn direct_builders_reject_forged_stream_capabilities_before_runtime_binding() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut prepared = registry
        .get("builtin.identity")
        .unwrap()
        .instantiate(json!({}))
        .unwrap();
    prepared
        .metadata
        .stream_outputs
        .push(mf_runtime::PortSpec::new(
            "delta",
            mf_runtime::ValueType::Usize,
            true,
        ));
    let node = mf_runtime::FlowNode::new("forged", prepared);
    let error =
        mf_compiler::FlowBuilder::prepare(vec![node], vec![], vec!["forged".into()], vec![])
            .err()
            .unwrap();
    assert!(matches!(
        error,
        mf_compiler::FlowBuildError::InvalidStreamPorts { .. }
    ));
}

fn stream_export() -> serde_json::Value {
    let mut document = root_export();
    document["execution"] =
        json!({"mode":"stream", "limits":{"workers":1,"max_pending_messages":2}});
    document["nodes"].as_array_mut().unwrap().push(json!({
        "id":"consume", "kind":"builtin.identity", "inputs":{"input":{"from":{"node":"group.count","port":"delta"}}}
    }));
    document
}

#[test]
fn combined_production_and_consumers_progress_with_one_task_worker() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for count in [0, 1000] {
        let document = serde_json::from_value(stream_export()).unwrap();
        let plan = compile_definition(&document, &registry).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let collected = events.clone();
        let instance = instantiate_stream(&plan, &registry)
            .unwrap()
            .start_with_options(StreamOptions {
                arguments: WorkflowArguments::from_json(
                    format!(r#"{{"group.count.count":{count}}}"#).as_bytes(),
                )
                .unwrap(),
                live: Some(mf_runtime::LivePublisher::new(move |event, _| {
                    collected.lock().unwrap().push(event);
                    Ok(())
                })),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            instance.recv().unwrap().unwrap().outputs["total"].as_u64(),
            Some(count)
        );
        assert!(instance.recv().unwrap().is_none());
        let summary = instance.join().unwrap();
        assert_eq!(summary.emitted_messages, count);
        assert_eq!(summary.completed_frames, count + 1);
        assert_eq!(events.lock().unwrap().len(), count as usize + 2);
    }
}

#[test]
fn empty_combined_invocation_does_not_close_later_input_invocations() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut document = stream_export();
    document["execution"]["limits"]["max_pending_messages"] = json!(3);
    document["nodes"][0]["inputs"] = json!({"count":{"from":{"node":"feed","port":"count"}}});
    document["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"feed","kind":"fixture.task_stream_feed","config":{"counts":[0,2]}}));
    let definition = serde_json::from_value(document).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start()
        .unwrap();
    for expected in [0, 2] {
        assert_eq!(
            instance.recv().unwrap().unwrap().outputs["total"].as_u64(),
            Some(expected)
        );
    }
    assert!(instance.recv().unwrap().is_none());
    assert_eq!(instance.join().unwrap().emitted_messages, 4);
}

#[test]
fn terminal_dependencies_progress_while_an_increment_is_unacknowledged() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("completed");
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut document = stream_export();
    document["outputs"] = json!([{"name":"delta","node":"consume","port":"value"}]);
    document["nodes"].as_array_mut().unwrap().push(json!({
        "id":"finish", "kind":"fixture.task_stream_finish", "config":{"path":marker},
        "inputs":{"total":{"from":{"node":"group.count","port":"total"}}}
    }));
    let definition = serde_json::from_value(document).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            arguments: WorkflowArguments::from_json(br#"{"group.count.count":1}"#).unwrap(),
            ..Default::default()
        })
        .unwrap();
    let delivery = instance.receive().unwrap().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !marker.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "1");
    delivery.acknowledge().unwrap();
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
}

#[test]
fn invalid_terminal_publication_cannot_turn_a_live_prefix_into_task_success() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut document = stream_export();
    document["nodes"][0]["kind"] = json!("fixture.task_stream_invalid_terminal");
    let plan = compile_definition(&serde_json::from_value(document).unwrap(), &registry).unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            arguments: WorkflowArguments::from_json(br#"{"group.count.count":1}"#).unwrap(),
            ..Default::default()
        })
        .unwrap();
    let error = instance.recv().unwrap_err();
    assert!(error.to_string().contains("invalid terminal output"));
    let mf_runtime::StreamError::Producer { source, .. } = error else {
        panic!("expected producer attribution")
    };
    let mf_runtime::NodeExecutionError::PluginFailed { source } = source.as_ref() else {
        panic!("expected publication cause")
    };
    assert!(matches!(
        source.downcast_ref::<mf_runtime::EmissionError>(),
        Some(mf_runtime::EmissionError::Terminal { .. })
    ));
    assert!(instance.join().is_err());
}

#[test]
fn combined_panics_retain_the_node_and_synthesized_runtime_source() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut document = stream_export();
    document["nodes"][0]["kind"] = json!("fixture.task_stream_panic");
    let plan = compile_definition(&serde_json::from_value(document).unwrap(), &registry).unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            arguments: WorkflowArguments::from_json(br#"{"group.count.count":1}"#).unwrap(),
            ..Default::default()
        })
        .unwrap();
    let error = instance.recv().unwrap_err();
    assert!(error.to_string().contains("group.count"));
    assert!(error.to_string().contains("combined panic sentinel"));
    let mf_runtime::StreamError::Producer { source, .. } = error else {
        panic!("expected producer attribution")
    };
    let mf_runtime::NodeExecutionError::PluginFailed { source } = source.as_ref() else {
        panic!("expected panic source")
    };
    assert!(source.downcast_ref::<mf_runtime::StreamError>().is_some());
    assert!(instance.join().is_err());
}

#[test]
fn disconnected_live_subscription_cancels_combined_production() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan =
        compile_definition(&serde_json::from_value(stream_export()).unwrap(), &registry).unwrap();
    let (publisher, receiver) = mf_runtime::live_channel(mf_runtime::LiveLimits {
        max_pending_records: 1,
        ..Default::default()
    })
    .unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            arguments: WorkflowArguments::from_json(br#"{"group.count.count":10000}"#).unwrap(),
            live: Some(publisher),
            ..Default::default()
        })
        .unwrap();
    assert!(matches!(
        receiver.recv().unwrap().unwrap().event,
        mf_runtime::LiveOutputKind::Open
    ));
    drop(receiver);
    assert!(
        instance
            .recv()
            .unwrap_err()
            .to_string()
            .contains("receiver disconnected")
    );
    assert!(instance.join().is_err());
}

#[test]
fn unconnected_producers_discard_without_reserving_delivery_capacity() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let document = json!({
        "version":"2026-10-23", "dependencies":{},
        "execution":{"mode":"stream","limits":{"workers":1,"max_pending_messages":1}},
        "nodes":[{"id":"feed","kind":"fixture.task_stream_feed","config":{"counts":[0,1,2,3]}}]
    });
    let plan = compile_definition(&serde_json::from_value(document).unwrap(), &registry).unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start()
        .unwrap();
    assert!(instance.recv().unwrap().is_none());
    let summary = instance.join().unwrap();
    assert_eq!(summary.emitted_messages, 4);
    assert_eq!(summary.completed_frames, 1);
}

#[test]
fn pure_producer_exports_drain_and_close_without_dropping_the_instance() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let document = json!({
        "version":"2026-10-23", "dependencies":{}, "execution":{"mode":"stream"},
        "nodes":[{"id":"feed","kind":"fixture.task_stream_feed","config":{"counts":[1,2]}}],
        "stream_outputs":[{"name":"counts","path":["feed"],"port":"count"}]
    });
    let plan = compile_definition(&serde_json::from_value(document).unwrap(), &registry).unwrap();
    let (publisher, receiver) = mf_runtime::live_channel(mf_runtime::LiveLimits {
        max_pending_records: 1,
        ..Default::default()
    })
    .unwrap();
    let (finished, ready) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().unwrap() {
            events.push(event);
        }
        finished.send(events).unwrap();
    });
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            live: Some(publisher),
            ..Default::default()
        })
        .unwrap();
    assert!(instance.recv().unwrap().is_none());
    let events = ready
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert_eq!(events.len(), 4);
    assert!(events.iter().all(|event| event.invocation.node == "feed"));
    instance.join().unwrap();
    reader.join().unwrap();
}

#[test]
fn event_live_exports_are_rejected_before_blocking_the_coordinator() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let document = json!({
        "version":"2026-10-23", "dependencies":{}, "execution":{"mode":"stream"},
        "nodes":[
            {"id":"feed","kind":"fixture.task_stream_feed","config":{"counts":[1]}},
            {"id":"collect","kind":"builtin.batch","config":{"max_items":1,"max_wait_ms":10},"inputs":{"item":{"from":{"node":"feed","port":"count"}}}}
        ],
        "stream_outputs":[{"name":"batch","path":["collect"],"port":"items"}]
    });
    let error =
        compile_definition(&serde_json::from_value(document).unwrap(), &registry).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Event results use ordinary stream output selection")
    );
}

#[test]
fn collector_timers_progress_while_combined_live_delivery_is_blocked() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut document = stream_export();
    document["execution"]["limits"]["max_pending_messages"] = json!(3);
    document["nodes"][1] = json!({
        "id":"consume", "kind":"builtin.batch", "config":{"max_items":10,"max_wait_ms":10},
        "inputs":{"item":{"from":{"node":"group.count","port":"delta"}}}
    });
    document["outputs"] = json!([{"name":"batch","node":"consume","port":"items"}]);
    let plan = compile_definition(&serde_json::from_value(document).unwrap(), &registry).unwrap();
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let blocked = gate.clone();
    let (started, ready) = std::sync::mpsc::channel();
    let publisher = mf_runtime::LivePublisher::new(move |event, cancellation| {
        if matches!(&event.event, mf_runtime::LiveOutputKind::Value { value } if value.as_u64() == Some(0))
        {
            started.send(()).unwrap();
            let (lock, changed) = &*blocked;
            let mut released = lock.lock().unwrap();
            while !*released {
                if let Some(error) = cancellation.failure() {
                    return Err(error);
                }
                released = changed
                    .wait_timeout(released, std::time::Duration::from_millis(20))
                    .unwrap()
                    .0;
            }
        }
        Ok(())
    });
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            arguments: WorkflowArguments::from_json(br#"{"group.count.count":2}"#).unwrap(),
            live: Some(publisher),
            ..Default::default()
        })
        .unwrap();
    ready
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let first = instance.recv().unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&first.outputs["batch"]).unwrap(),
        json!([0])
    );
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let second = instance.recv().unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&second.outputs["batch"]).unwrap(),
        json!([1])
    );
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
}

#[test]
fn combined_observation_reports_terminal_ports_even_without_emissions() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for count in [0, 2] {
        let harness = capture::Harness::new(true);
        let plan = compile_definition(&serde_json::from_value(stream_export()).unwrap(), &registry)
            .unwrap();
        let observation = plan
            .start_stream_observation(&harness.observer(), mf_telemetry::identity::RunId::new())
            .unwrap();
        let instance = instantiate_stream(&plan, &registry)
            .unwrap()
            .start_with_options(StreamOptions {
                arguments: WorkflowArguments::from_json(
                    format!(r#"{{"group.count.count":{count}}}"#).as_bytes(),
                )
                .unwrap(),
                observation: Some(observation),
                ..Default::default()
            })
            .unwrap();
        instance.recv().unwrap().unwrap();
        assert!(instance.recv().unwrap().is_none());
        instance.join().unwrap();
        let records = harness
            .records()
            .iter()
            .filter(|record| record.scope == mf_telemetry::INSTRUMENTATION_SCOPE)
            .map(|record| mf_telemetry::stream::StreamRecord::decode(record).unwrap())
            .collect::<Vec<_>>();
        let ports = records
            .iter()
            .find_map(|record| match &record.payload {
                mf_telemetry::stream::StreamPayload::Execution(
                    mf_telemetry::event::Event::NodeFinished {
                        node,
                        produced_ports,
                        ..
                    },
                ) if node.id == "group.count" => Some(produced_ports),
                _ => None,
            })
            .unwrap();
        assert!(ports.iter().any(|port| port == "total"));
        assert_eq!(ports.iter().any(|port| port == "delta"), count > 0);
        assert!(!harness.spans.get_finished_spans().unwrap().is_empty());
    }
}

#[test]
fn skipped_combined_nodes_do_not_invent_live_invocations() {
    let document = json!({
        "version":"2026-10-23", "dependencies":{},
        "nodes":[
            {"id":"seed","kind":"builtin.constant","config":{"value":2}},
            {"id":"route","kind":"builtin.if_else","config":{"branches":[{
                "id":"selected","condition":{"source":{"output":"seed.value","path":""},"operator":"eq","value":3}
            }]}},
            {"id":"count","kind":"fixture.task_stream","inputs":{"count":{"from":{"node":"seed","port":"value"}}}}
        ],
        "edges":[{"from_node":"seed","to_node":"route"},{"from_node":"route","from_output":"selected","to_node":"count"}],
        "outputs":[{"name":"total","node":"count","port":"total","optional":true}],
        "stream_outputs":[{"name":"increments","path":["count"],"port":"delta"}]
    });
    let (outputs, events) = run_live(document, "{}");
    assert!(outputs.is_empty());
    assert!(events.is_empty());
}
