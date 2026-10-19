#[path = "fixtures/observation_capture.rs"]
mod capture;
mod common;
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

fn exclusive_success_graph(value: i64, path: &Path) -> WorkflowDefinition {
    definition(
        vec![
            json!({"id":"value","kind":"builtin.constant","config":{"value":value}}),
            router("value.value"),
            json!({"id":"left","kind":"builtin.constant","config":{"value":11}}),
            json!({"id":"right","kind":"builtin.constant","config":{"value":22}}),
            json!({"id":"select","kind":"builtin.identity","inputs":{"input":{"exclusive":[
                {"node":"left","port":"value"},{"node":"right","port":"value"}
            ]}}}),
            task("after_select", path),
            task("chained", path),
            task("positive", path),
            task("joined", path),
        ],
        json!([
            {"from_node":"value","to_node":"route"},
            {"from_node":"route","from_output":"positive","to_node":"left"},
            {"from_node":"route","from_output":"else","to_node":"right"},
            {"from_node":"select","to_node":"after_select"},
            {"from_node":"after_select","to_node":"chained"},
            {"from_node":"route","from_output":"positive","to_node":"positive"},
            {"from_node":"chained","to_node":"joined"},{"from_node":"positive","to_node":"joined"}
        ]),
    )
}

#[test]
fn compiled_success_conditions_preserve_exclusive_alternatives_and_snapshot_skips() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("trace");
    let registry = NodeRegistry::from_inventory().unwrap();
    for value in [-1, 1] {
        let graph = exclusive_success_graph(value, &path);
        let flow = instantiate_compiled(&compile_definition(&graph, &registry).unwrap(), &registry)
            .unwrap();
        for snapshots in [false, true] {
            let _ = std::fs::remove_file(&path);
            let mut ctx = mf_runtime::ExecutionContext::default();
            let recording = mf_runtime::SnapshotRecording::memory();
            if snapshots {
                ctx.set_snapshot_recorder(recording.recorder());
            }
            flow.execute_in_context(&mut ctx).unwrap();
            let mut calls = trace(&path);
            calls.sort();
            assert_eq!(
                calls,
                if value > 0 {
                    vec!["after_select", "chained", "joined", "positive"]
                } else {
                    vec!["after_select", "chained"]
                }
            );
            if snapshots {
                assert!(!recording.history().is_empty());
            }
        }
        std::fs::remove_file(&path).unwrap();
        let mut cancelled = mf_runtime::ExecutionContext::default();
        cancelled
            .cancellation()
            .cancel(mf_runtime::StreamError::Execution {
                message: "cancelled before activation".into(),
            });
        assert!(flow.execute_in_context(&mut cancelled).is_err());
        assert!(trace(&path).is_empty());
    }
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
fn stream_messages_have_independent_success_conditions() {
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
fn body_invocations_keep_success_conditions_local_and_containers_are_rejected() {
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

#[test]
fn success_observations_round_trip_and_reject_protocol_downgrades() {
    use mf_telemetry::{
        event::{Event, SkipCause},
        identity::RunId,
    };
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
            {"from_node":"value","to_node":"route"},{"from_node":"route","from_output":"positive","to_node":"source"},{"from_node":"source","to_node":"target"}
        ]),
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&graph, &registry).unwrap();
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    assert_eq!(
        description.event_schema_version(),
        mf_telemetry::SUCCESS_EVENT_SCHEMA_VERSION
    );
    let harness = capture::Harness::new(false);
    let observation = plan
        .start_observation(&harness.observer(), RunId::new())
        .unwrap();
    mf_compiler::execute_compiled(&plan, &registry, Some(observation)).unwrap();
    let records = harness.records();
    assert!(!records.is_empty());
    assert!(harness.spans.get_finished_spans().unwrap().is_empty());
    let mut seen = false;
    for wire in records {
        assert_eq!(
            wire.schema_version().unwrap(),
            mf_telemetry::SUCCESS_EVENT_SCHEMA_VERSION
        );
        let event = wire.decode().unwrap();
        event.validate_for(&description).unwrap();
        if let Event::NodeSkipped { node, causes, .. } = event.event
            && node.id == "target"
        {
            assert_eq!(
                causes,
                [SkipCause::NodeSuccess {
                    node_success: "source".into()
                }]
            );
            assert!(wire.body["causes"][0].get("source_output").is_none());
            let mut old = wire.clone();
            old.attributes.insert("mf.schema.version".into(), json!(2));
            assert!(old.decode().is_err());
            seen = true;
        }
    }
    assert!(seen);
    let mut old = description.clone();
    old.version = mf_telemetry::description::WorkflowDescriptionVersion::V2026_10_03;
    assert!(old.validate().is_err());
    let mut malformed = description;
    malformed.order_edges[0].to_node = "unknown".into();
    assert!(malformed.validate().is_err());
}

#[test]
fn stream_success_events_round_trip_with_node_only_skip_causes() {
    use mf_telemetry::{
        event::{Event, SkipCause},
        identity::RunId,
        stream::{StreamPayload, StreamRecord},
    };
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
            {"from_node":"feed","from_output":"item","to_node":"route"},{"from_node":"route","from_output":"positive","to_node":"source"},{"from_node":"source","to_node":"target"}
        ]),
    );
    graph.execution = Some(serde_json::from_value(json!({"mode":"stream"})).unwrap());
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&graph, &registry).unwrap();
    let harness = capture::Harness::new(false);
    let observation = plan
        .start_stream_observation(&harness.observer(), RunId::new())
        .unwrap();
    let run = controlled::SourceRun::start(
        mf_compiler::instantiate_stream(&plan, &registry).unwrap(),
        "feed",
        mf_runtime::StreamOptions {
            observation: Some(observation),
            ..Default::default()
        },
    )
    .unwrap();
    run.source.send(json!(-1)).unwrap();
    run.source.close();
    while run.recv().unwrap().is_some() {}
    run.join().unwrap();
    let mut seen = false;
    for wire in harness.records() {
        let record = StreamRecord::decode(&wire).unwrap();
        assert_eq!(
            record.schema_version,
            mf_telemetry::SUCCESS_STREAM_EVENT_SCHEMA_VERSION
        );
        let encoded = record
            .to_wire(wire.time_unix_nano, wire.trace_context.clone())
            .unwrap();
        assert_eq!(StreamRecord::decode(&encoded).unwrap(), record);
        if let StreamPayload::Execution(Event::NodeSkipped { node, causes, .. }) = record.payload
            && node.id == "target"
        {
            assert_eq!(
                causes,
                [SkipCause::NodeSuccess {
                    node_success: "source".into()
                }]
            );
            let mut old = wire;
            old.attributes.insert("mf.schema.version".into(), json!(4));
            assert!(StreamRecord::decode(&old).is_err());
            seen = true;
        }
    }
    assert!(seen);
}

#[test]
fn success_edges_change_identity_even_when_execution_order_is_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("trace");
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut graph = definition(
        vec![task("a", &path), task("z", &path)],
        json!([{"from_node":"a","to_node":"z"}]),
    );
    let with = compile_definition(&graph, &registry).unwrap();
    let id = with.manifest(&registry).unwrap().workflow_id().unwrap();
    graph.order_edges.clear();
    let without = compile_definition(&graph, &registry).unwrap();
    assert_eq!(with.execution_order, without.execution_order);
    let manifest = without.manifest(&registry).unwrap();
    assert_ne!(id, manifest.workflow_id().unwrap());
    assert_eq!(
        manifest.description().unwrap().event_schema_version(),
        mf_telemetry::LOOP_EVENT_SCHEMA_VERSION
    );
}

#[test]
fn standalone_success_dependencies_match_memory_and_preserve_manifests() {
    use std::{fs, process::Command};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("trace");
    let project = root.path().join("build");
    let input = root.path().join("lines");
    fs::write(&input, "first\nsecond\n").unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut cases = vec![definition(
        vec![task("z_write", &path), task("a_notify", &path)],
        json!([{"from_node":"z_write","to_node":"a_notify"}]),
    )];
    const DENY_SUCCESS_ENV: &str = "MF_TEST_SUCCESS_CONTRACT_DRIFT";
    cases[0].nodes[0].config["deny_success_env"] = json!(DENY_SUCCESS_ENV);
    cases.extend([-1, 1].map(|value| exclusive_success_graph(value, &path)));
    let mut value = task("value", &path);
    value["config"]["ports"] = json!(["value"]);
    value["config"]["outputs"] = json!({"value":42});
    let mut typed = definition(
        vec![
            value,
            json!({"id":"source","kind":"builtin.identity","inputs":{"input":{"from":{"node":"value","port":"value"}}}}),
            json!({"id":"target","kind":"builtin.identity","inputs":{"input":{"from":{"node":"source","port":"value"}}}}),
            task("after_typed", &path),
            task("peer", &path),
        ],
        json!([{"from_node":"source","to_node":"target"},{"from_node":"target","to_node":"after_typed"},{"from_node":"target","to_node":"peer"}]),
    );
    typed.outputs =
        serde_json::from_value(json!([{"name":"result","node":"target","port":"value"}])).unwrap();
    cases.push(typed);
    for source in [
        include_str!("../../../examples/loop.json"),
        include_str!("../../../examples/iteration.json"),
    ] {
        let mut graph: WorkflowDefinition = serde_json::from_str(source).unwrap();
        graph.version = mf_compiler::WorkflowDefinitionVersion::V2026_10_19;
        if let Some(definition) = &mut graph.nodes[1].loop_definition {
            definition.body.nodes.extend([
                serde_json::from_value(task("a", &path)).unwrap(),
                serde_json::from_value(task("b", &path)).unwrap(),
            ]);
            definition
                .body
                .order_edges
                .push(mf_compiler::OrderEdgeDefinition {
                    from_node: "a".into(),
                    to_node: "b".into(),
                });
        } else {
            graph.nodes[1].config["body"]["nodes"]
                .as_array_mut()
                .unwrap()
                .extend([task("a", &path), task("b", &path)]);
            graph.nodes[1].config["body"]["order_edges"] = json!([{"from_node":"a","to_node":"b"}]);
        }
        cases.push(graph);
    }
    let mut stream = definition(
        vec![
            json!({"id":"read","kind":"builtin.readline"}),
            task("source", &path),
            task("target", &path),
        ],
        json!([
            {"from_node":"read","from_output":"line","to_node":"source"},{"from_node":"source","to_node":"target"}
        ]),
    );
    stream.execution = Some(serde_json::from_value(json!({"mode":"stream"})).unwrap());
    stream.outputs =
        serde_json::from_value(json!([{"name":"line","node":"read","port":"line"}])).unwrap();
    cases.push(stream);
    for mut graph in cases {
        let _ = fs::remove_file(&path);
        graph.dependencies=serde_json::from_value(json!({
            "fixture":{"package":"fixture-multi-nodes","path":common::crates_dir().join("mf-compiler/tests/fixtures/multi-nodes")},
            "core":{"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")},
            "code":{"package":"mfn-code","path":common::crates_dir().join("builtin-nodes/code")}
        })).unwrap();
        let plan = compile_definition(&graph, &registry).unwrap();
        if graph.nodes.iter().any(|node| node.id.as_str() == "peer") {
            let artifacts = plan.generate_runner_execution_plans(&registry).unwrap();
            let typed: mf_compiler::TypedPlan =
                serde_json::from_str(&artifacts.typed_plan_json).unwrap();
            assert!(typed.connections.iter().any(|edge| edge.source == "source"
                && edge.target == "target"
                && edge.fallback.is_none()));
            assert!(!typed.segments.is_empty());
        }
        let manifest = plan.manifest(&registry).unwrap();
        assert_eq!(
            mf_runtime::WorkflowManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap(),
            manifest
        );
        let description = manifest.description().unwrap();
        description.validate().unwrap();
        assert!(
            !description.order_edges.is_empty()
                || description
                    .loop_bodies
                    .iter()
                    .any(|body| !body.order_edges.is_empty())
        );
        let expected: Vec<Value> = if graph.execution.is_some() {
            let instance = mf_compiler::instantiate_stream(&plan, &registry)
                .unwrap()
                .start_with_options(mf_runtime::StreamOptions {
                    arguments: mf_runtime::WorkflowArguments::try_from(json!({"read.path":input}))
                        .unwrap(),
                    ..Default::default()
                })
                .unwrap();
            let mut outputs = Vec::new();
            while let Some(value) = instance.recv().unwrap() {
                outputs.push(json!(value.outputs));
            }
            instance.join().unwrap();
            outputs
        } else {
            vec![json!(
                mf_compiler::execute_compiled(&plan, &registry, None).unwrap()
            )]
        };
        let expected_trace = trace(&path);
        fs::remove_file(&path).unwrap();
        mf_compiler::write_dependency_project(
            &project,
            &plan,
            &mf_compiler::SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        )
        .unwrap();
        mf_compiler::resolve_project(&project, &root.path().join("flow.lock"), false).unwrap();
        let build = mf_compiler::cargo_command(&project)
            .args(["build", "--release", "--offline", "--locked"])
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        assert!(!path.exists());
        if graph
            .nodes
            .iter()
            .any(|node| node.config.get("deny_success_env").is_some())
        {
            let rejected = Command::new(common::runner_executable(&project, "release"))
                .env(DENY_SUCCESS_ENV, "deny")
                .output()
                .unwrap();
            assert!(!rejected.status.success());
            assert!(String::from_utf8_lossy(&rejected.stderr).contains("ordinary Tasks"));
            assert!(
                !path.exists(),
                "preparation must reject drift before any side effect"
            );
        }
        let mut command = Command::new(common::runner_executable(&project, "release"));
        if graph.execution.is_some() {
            command.arg("--read.path").arg(&input);
        }
        let result = command.arg("-v").output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let actual: Vec<Value> = result
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(actual, expected);
        let actual_trace = trace(&path);
        if graph
            .nodes
            .iter()
            .any(|node| matches!(node.id.as_str(), "select" | "peer"))
        {
            // Independent domains may report their side effects in either order.
            let mut expected = expected_trace;
            let mut actual = actual_trace.clone();
            expected.sort();
            actual.sort();
            assert_eq!(actual, expected);
            for edge in &graph.order_edges {
                if let (Some(source), Some(target)) = (
                    actual_trace
                        .iter()
                        .position(|id| id == edge.from_node.as_str()),
                    actual_trace
                        .iter()
                        .position(|id| id == edge.to_node.as_str()),
                ) {
                    assert!(source < target);
                }
            }
        } else {
            assert_eq!(actual_trace, expected_trace);
        }
    }
}
