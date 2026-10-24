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

#[derive(serde::Deserialize)]
struct PortDeclarations {
    outputs: Vec<String>,
    stream_outputs: Vec<String>,
}

impl mf_runtime::NodePortContract for PortDeclarations {
    fn ports(&self) -> mf_runtime::NodePorts {
        mf_runtime::NodePorts {
            inputs: Vec::new(),
            outputs: self
                .outputs
                .iter()
                .map(|name| {
                    mf_runtime::PortSpec::owned(name.clone(), mf_runtime::ValueType::Int64, true)
                })
                .collect(),
        }
    }

    fn stream_outputs(&self) -> Vec<mf_runtime::PortSpec> {
        self.stream_outputs
            .iter()
            .map(|name| {
                mf_runtime::PortSpec::owned(name.clone(), mf_runtime::ValueType::Int64, true)
            })
            .collect()
    }
}

impl mf_runtime::TaskStreamNode for PortDeclarations {
    fn execute(
        &self,
        _: mf_runtime::Inputs,
        _: &mut mf_runtime::ExecutionContext,
        _: &mut mf_runtime::Emitter<'_>,
    ) -> Result<mf_runtime::NodeResult, mf_runtime::NodeExecutionError> {
        panic!("invalid declarations must be rejected before execution")
    }
}

fn declared_ports(
    config: serde_json::Value,
) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    let declarations: PortDeclarations = mf_runtime::deserialize_config(config)?;
    mf_runtime::PreparedNode::task_stream(declarations, mf_runtime::NodeMetadata::default())
}

inventory::submit! {
    mf_runtime::NodeRegistration {
        kind: "test.task_stream_declarations",
        factory: mf_runtime::NodeFactory::Plain(declared_ports),
    }
}

#[test]
fn compiler_rejects_duplicate_empty_and_colliding_output_declarations() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (outputs, stream_outputs, diagnostic) in [
        (
            vec!["total"],
            vec!["delta", "delta"],
            "duplicate stream output port `delta`",
        ),
        (
            vec!["total"],
            vec![""],
            "empty or duplicate stream output port ``",
        ),
        (
            vec!["delta"],
            vec!["delta"],
            "terminal and stream outputs share port `delta`",
        ),
        (
            vec!["total", "total"],
            vec!["delta"],
            "duplicate output port `total`",
        ),
    ] {
        let config = json!({"outputs":outputs, "stream_outputs":stream_outputs});
        registry
            .get("test.task_stream_declarations")
            .unwrap()
            .instantiate(config.clone())
            .unwrap();
        let definition: WorkflowDefinition = serde_json::from_value(json!({
            "version":"2026-10-19", "dependencies":{},
            "nodes":[{"id":"invalid", "kind":"test.task_stream_declarations", "config":config}],
            "outputs":[]
        }))
        .unwrap();
        let error = compile_definition(&definition, &registry).unwrap_err();
        assert!(
            matches!(error, mf_compiler::WorkflowCompileError::InvalidNodeMetadata { ref definition_id, ref message }
                if definition_id.as_str() == "invalid" && message.contains(diagnostic)),
            "{error}"
        );
    }
}

fn root_export() -> serde_json::Value {
    json!({
        "version":"2026-10-23", "dependencies":{},
        "execution":{"mode":"stream","limits":{"workers":1,"max_pending_messages":2}},
        "nodes":[{"id":"group.count","kind":"fixture.task_stream"}],
        "outputs":[{"name":"total","node":"group.count","port":"total"},
            {"name":"increment","node":"group.count","port":"delta"}]
    })
}

fn stream_export() -> serde_json::Value {
    let mut document = root_export();
    document["nodes"].as_array_mut().unwrap().push(json!({
        "id":"consume","kind":"builtin.identity",
        "inputs":{"input":{"from":{"node":"group.count","port":"delta"}}}
    }));
    document["outputs"] = json!([{"name":"increment","node":"consume","port":"value"}]);
    document
}

#[test]
fn selected_incremental_and_terminal_outputs_keep_their_message_domains() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for count in [0, 1000] {
        let plan =
            compile_definition(&serde_json::from_value(root_export()).unwrap(), &registry).unwrap();
        let instance = instantiate_stream(&plan, &registry)
            .unwrap()
            .start_with_options(StreamOptions {
                arguments: WorkflowArguments::from_json(
                    format!(r#"{{"group.count.count":{count}}}"#).as_bytes(),
                )
                .unwrap(),
                ..Default::default()
            })
            .unwrap();
        let mut increments = Vec::new();
        let mut terminal = 0;
        while let Some(output) = instance.recv().unwrap() {
            if let Some(value) = output.outputs.get("total") {
                assert_eq!(output.message.domain, 0);
                assert_eq!(value.as_u64(), Some(count));
                terminal += 1;
            } else {
                assert_ne!(output.message.domain, 0);
                increments.push(output.outputs["increment"].as_u64().unwrap());
            }
        }
        assert_eq!(terminal, 1);
        assert_eq!(increments, (0..count).collect::<Vec<_>>());
        instance.join().unwrap();
    }
}

#[test]
fn independent_selected_domains_do_not_overwrite_pending_deliveries() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition = serde_json::from_value(json!({
        "version":"2026-10-23", "dependencies":{}, "execution":{"mode":"stream"},
        "nodes":[
            {"id":"first","kind":"fixture.task_stream_feed","config":{"counts":[1,2,3]}},
            {"id":"second","kind":"fixture.task_stream_feed","config":{"counts":[4,5,6]}}
        ],
        "outputs":[{"name":"first","node":"first","port":"count"},
            {"name":"second","node":"second","port":"count"}]
    }))
    .unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start()
        .unwrap();
    let mut first = Vec::new();
    let mut second = Vec::new();
    while let Some(delivery) = instance.receive().unwrap() {
        let output = &delivery.output().outputs;
        assert_eq!(output.len(), 1);
        if let Some(value) = output.get("first") {
            first.push(value.as_u64().unwrap());
        }
        if let Some(value) = output.get("second") {
            second.push(value.as_u64().unwrap());
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
        delivery.acknowledge().unwrap();
    }
    assert_eq!(first, [1, 2, 3]);
    assert_eq!(second, [4, 5, 6]);
    let summary = instance.join().unwrap();
    assert_eq!(summary.delivered_outputs, 6);
    assert_eq!(summary.completed_frames, 7);
}

#[test]
fn selected_domains_wait_for_acknowledgement_before_the_next_delivery() {
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};

    let root = tempfile::tempdir().unwrap();
    let release = root.path().join("release");
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition = serde_json::from_value(json!({
        "version":"2026-10-19", "dependencies":{}, "execution":{"mode":"stream"},
        "nodes":[
            {"id":"first","kind":"fixture.task_stream","config":{"wait_path":release}},
            {"id":"second","kind":"fixture.task_stream","config":{"wait_path":release}}
        ],
        "outputs":[{"name":"first","node":"first","port":"delta"},
            {"name":"second","node":"second","port":"delta"}]
    }))
    .unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let instance = Arc::new(
        instantiate_stream(&plan, &registry)
            .unwrap()
            .start_with_options(StreamOptions {
                arguments: WorkflowArguments::from_json(br#"{"first.count":1,"second.count":1}"#)
                    .unwrap(),
                ..Default::default()
            })
            .unwrap(),
    );
    let first = instance.receive().unwrap().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while instance.metrics().pending_frames < 3 {
        assert!(
            Instant::now() < deadline,
            "both output domains must become ready"
        );
        std::thread::yield_now();
    }
    std::fs::write(release, "release").unwrap();
    while instance.metrics().active_workers > 0 {
        assert!(Instant::now() < deadline, "both producers must finish");
        std::thread::yield_now();
    }
    let (ready, started) = mpsc::sync_channel(1);
    let (sender, receiver) = mpsc::sync_channel(1);
    let receiving = Arc::clone(&instance);
    let reader = std::thread::spawn(move || {
        ready.send(()).unwrap();
        assert!(sender.send(receiving.receive()).is_ok());
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    first.acknowledge().unwrap();
    let second = receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap()
        .unwrap();
    second.acknowledge().unwrap();
    reader.join().unwrap();
    assert!(instance.recv().unwrap().is_none());
    let summary = Arc::try_unwrap(instance).ok().unwrap().join().unwrap();
    assert_eq!(summary.delivered_outputs, 2);
    assert_eq!(summary.completed_frames, 3);
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
fn combined_terminal_results_support_output_and_success_dependencies() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-10-23", "dependencies": {},
        "nodes": [
            {"id":"source", "kind":"fixture.task_stream"},
            {"id":"after", "kind":"fixture.task_stream", "inputs": {
                "count":{"from":{"node":"source", "port":"total"}}
            }}
        ],
        "edges": [{"from_node":"source", "to_node":"after"}],
        "outputs": [{"name":"total", "node":"after", "port":"total"}]
    }))
    .unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let outputs = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_with_inputs(WorkflowArguments::from_json(br#"{"source.count":3}"#).unwrap())
        .unwrap();
    assert_eq!(outputs["total"].as_u64(), Some(3));
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
    let error = loop {
        match instance.recv() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("expected execution failure"),
            Err(error) => break error,
        }
    };
    assert_eq!(error.phase(), "output");
    let mf_runtime::StreamError::Producer { source, .. } = error else {
        panic!("expected producer attribution")
    };
    let mf_runtime::NodeExecutionError::PluginFailed { source } = source.as_ref() else {
        panic!("expected publication cause")
    };
    assert!(matches!(
        source.downcast_ref::<mf_runtime::WorkflowRunError>(),
        Some(mf_runtime::WorkflowRunError::OutputType { definition_id, .. })
            if definition_id.as_str() == "group.count"
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
    let error = loop {
        match instance.recv() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("expected execution failure"),
            Err(error) => break error,
        }
    };
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
        while instance.recv().unwrap().is_some() {}
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
fn finite_output_selection_cannot_consume_an_incremental_port() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = root_export();
    value.as_object_mut().unwrap().remove("execution");
    let error = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap_err();
    assert!(error.to_string().contains("streaming execution"));
}
