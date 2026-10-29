#[allow(dead_code)]
#[path = "fixtures/observation_capture.rs"]
mod capture;
mod common;
#[path = "fixtures/multi-nodes/src/task_stream.rs"]
mod fixture;
extern crate mfn_core as _;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled, instantiate_stream,
};
use mf_runtime::{StreamError, StreamOptions, WorkflowArguments};
use serde_json::{Value, json};

fn definition() -> Value {
    json!({"version":"2026-10-28","dependencies":{},
        "nodes":[{"id":"repeat","kind":"workflow.loop","loop":{
            "max_iterations":3,"variables":[{"name":"count","type":"usize"}],
            "stream_outputs":{"increment":{"node":"step","port":"delta"}},
            "body":{"nodes":[{"id":"step","kind":"fixture.task_stream","inputs":{"count":{"from":{"node":"%loop","port":"count"}}}}]}
        }}],
        "outputs":[{"name":"increment","node":"repeat","port":"increment"},{"name":"count","node":"repeat","port":"count"}]
    })
}

fn prepare(value: Value) -> mf_runtime::PreparedStream {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    instantiate_stream(&plan, &registry).unwrap()
}
fn options() -> StreamOptions {
    StreamOptions {
        arguments: WorkflowArguments::from_json(br#"{"repeat.count":2}"#).unwrap(),
        ..Default::default()
    }
}

#[test]
fn loop_forwards_child_emissions_in_order_and_completes_once_with_its_summary() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut definition = definition();
    definition["execution"] = json!({"limits":{"workers":1,"max_pending_messages":2}});
    let plan = compile_definition(&serde_json::from_value(definition).unwrap(), &registry).unwrap();
    let harness = capture::Harness::new(true);
    let observation = plan
        .start_stream_observation(&harness.observer(), mf_telemetry::identity::RunId::new())
        .unwrap();
    let mut options = options();
    options.observation = Some(observation);
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(options)
        .unwrap();
    let mut values = Vec::new();
    while let Some(output) = instance.recv().unwrap() {
        values.push(serde_json::to_value(output.outputs).unwrap());
    }
    instance.join().unwrap();
    let increments: Vec<_> = values
        .iter()
        .filter_map(|value| value.get("increment"))
        .cloned()
        .collect();
    assert_eq!(
        increments,
        vec![json!(0), json!(1), json!(0), json!(1), json!(0), json!(1)]
    );
    let terminal: Vec<_> = values
        .iter()
        .filter(|value| value.get("count").is_some())
        .cloned()
        .collect();
    assert_eq!(terminal, vec![json!({"count":2})]);
    let records: Vec<_> = harness
        .records()
        .iter()
        .filter(|record| record.scope == mf_telemetry::INSTRUMENTATION_SCOPE)
        .map(|record| mf_telemetry::stream::StreamRecord::decode(record).unwrap())
        .collect();
    let summaries: Vec<_> = records
        .iter()
        .filter_map(|record| match &record.payload {
            mf_telemetry::stream::StreamPayload::Execution(
                mf_telemetry::event::Event::NodeFinished {
                    node, loop_summary, ..
                },
            ) if node.id == "repeat" && node.path.is_empty() => Some(loop_summary),
            _ => None,
        })
        .collect();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].as_ref().unwrap().pass_count.get(), 3);
}

#[test]
fn nested_loops_compose_stream_exports_without_replacing_loop_variables() {
    let mut value = definition();
    let inner = value["nodes"][0].clone();
    let mut inner = inner;
    inner["id"] = "inner".into();
    inner["loop"]["max_iterations"] = 2.into();
    inner["inputs"] = json!({"count":{"from":{"node":"%loop","port":"count"}}});
    value["nodes"][0]["loop"]["max_iterations"] = 2.into();
    value["nodes"][0]["loop"]["body"]["nodes"] = json!([inner]);
    value["nodes"][0]["loop"]["stream_outputs"] =
        json!({"increment":{"node":"inner","port":"increment"}});
    let instance = prepare(value).start_with_options(options()).unwrap();
    let mut increments = Vec::new();
    let mut terminals = 0;
    while let Some(output) = instance.recv().unwrap() {
        if let Some(value) = output.outputs.get("increment") {
            increments.push(value.as_u64().unwrap());
        } else {
            terminals += 1;
            assert_eq!(output.outputs["count"], json!(2));
        }
    }
    instance.join().unwrap();
    assert_eq!(increments, vec![0, 1, 0, 1, 0, 1, 0, 1]);
    assert_eq!(terminals, 1);
}

#[test]
fn a_child_prefix_arrives_before_completion_and_cancellation_stops_remaining_passes() {
    for cancelled in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let release = root.path().join("release");
        let mut value = definition();
        value["nodes"][0]["loop"]["body"]["nodes"][0]["config"] = json!({"wait_path":release});
        let instance = prepare(value).start_with_options(options()).unwrap();
        let cancellation = instance.cancellation();
        let first = instance.receive().unwrap().unwrap();
        assert_eq!(first.output().outputs["increment"], json!(0));
        first.acknowledge().unwrap();
        if cancelled {
            cancellation.cancel(
                mf_runtime::StreamExecutionSnafu {
                    message: "stop after prefix",
                }
                .build(),
            );
            assert!(instance.join().is_err());
        } else {
            std::fs::write(release, "continue").unwrap();
            let mut remaining = 0;
            while instance.recv().unwrap().is_some() {
                remaining += 1;
            }
            assert_eq!(remaining, 6);
            instance.join().unwrap();
        }
    }
}

#[test]
fn stream_export_declarations_reject_old_versions_unknown_children_ports_and_collisions() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut old = definition();
    old["version"] = "2026-10-25".into();
    assert!(serde_json::from_value::<WorkflowDefinition>(old).is_err());
    for exports in [
        json!({"increment":{"node":"missing","port":"delta"}}),
        json!({"increment":{"node":"step","port":"total"}}),
        json!({"count":{"node":"step","port":"delta"}}),
        json!({"":{"node":"step","port":"delta"}}),
    ] {
        let mut value = definition();
        value["nodes"][0]["loop"]["stream_outputs"] = exports;
        assert!(compile_definition(&serde_json::from_value(value).unwrap(), &registry).is_err());
    }
}

#[test]
fn child_delivery_errors_stop_forwarding_and_keep_typed_sources() {
    use std::error::Error;
    struct Reject(usize);
    impl mf_runtime::EmissionSink for Reject {
        fn send(&mut self, _: mf_runtime::NodeResult) -> Result<(), StreamError> {
            self.0 += 1;
            mf_runtime::StreamExecutionSnafu {
                message: "delivery sentinel",
            }
            .fail()
        }
    }
    let mut value = definition();
    value["outputs"] = json!([{"name":"count","node":"repeat","port":"count"}]);
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let node = &flow.nodes()[0];
    let mut sink = Reject(0);
    let error = node
        .node
        .execute_with_sink(
            [("count".into(), 2_u64.into())].into(),
            &mut mf_runtime::ExecutionContext::default(),
            &mut sink,
        )
        .unwrap_err();
    assert_eq!(sink.0, 1);
    let mut source: &dyn Error = &error;
    let mut typed = false;
    while let Some(next) = source.source() {
        typed |= next.is::<StreamError>();
        source = next;
    }
    assert!(typed);
    assert!(error.to_string().contains("delivery sentinel"));
}

#[test]
fn multiple_child_exports_preserve_sparse_packet_ownership() {
    let mut value = definition();
    let mut other = value["nodes"][0]["loop"]["body"]["nodes"][0].clone();
    other["id"] = "other".into();
    value["nodes"][0]["loop"]["body"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(other);
    value["nodes"][0]["loop"]["stream_outputs"]["second"] = json!({"node":"other","port":"delta"});
    value["outputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"second","node":"repeat","port":"second"}));
    for output in value["outputs"].as_array_mut().unwrap() {
        if output["name"] != "count" {
            output["optional"] = true.into();
        }
    }
    let instance = prepare(value).start_with_options(options()).unwrap();
    let mut first = vec![];
    let mut second = vec![];
    let mut terminal = 0;
    while let Some(output) = instance.recv().unwrap() {
        assert_eq!(output.outputs.len(), 1);
        if let Some(value) = output.outputs.get("increment") {
            first.push(value.as_u64().unwrap());
        } else if let Some(value) = output.outputs.get("second") {
            second.push(value.as_u64().unwrap());
        } else {
            terminal += 1;
        }
    }
    instance.join().unwrap();
    assert_eq!(first, vec![0, 1, 0, 1, 0, 1]);
    assert_eq!(second, first);
    assert_eq!(terminal, 1);
}

fn guarded_definition(enabled: bool, marker: &std::path::Path) -> Value {
    let mut value = definition();
    let mut step = value["nodes"][0]["loop"]["body"]["nodes"][0].clone();
    step["config"] = json!({"marker":marker});
    value["nodes"][0]["loop"]["body"] = json!({"nodes":[
        {"id":"enabled","kind":"builtin.constant","config":{"value":enabled}},
        {"id":"gate","kind":"builtin.if_else","config":{"branches":[{"id":"pass","condition":{"source":{"output":"enabled.value","path":""},"operator":"eq","value":true}}]}},
        {"id":"ready","kind":"builtin.identity","inputs":{"input":{"from":{"node":"gate","port":"pass"}}}},step
    ],"edges":[{"from_node":"enabled","from_output":"value","to_node":"gate"},{"from_node":"ready","to_node":"step"}]});
    value
}

#[test]
fn exported_children_reuse_success_guards_in_streamed_and_terminal_execution() {
    let root = tempfile::tempdir().unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    for enabled in [false, true] {
        for streamed in [false, true] {
            let marker = root.path().join(format!("{enabled}-{streamed}"));
            let mut value = guarded_definition(enabled, &marker);
            if !streamed {
                value["outputs"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|output| output["name"] != "increment");
                value["nodes"][0]["loop"]["body"]["nodes"][3]["config"]["wait_path"] =
                    json!(root.path().join("unreleased"));
            }
            let plan =
                compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
            if streamed {
                let instance = instantiate_stream(&plan, &registry)
                    .unwrap()
                    .start_with_options(options())
                    .unwrap();
                let mut increments = 0;
                while let Some(output) = instance.recv().unwrap() {
                    increments += usize::from(output.outputs.contains_key("increment"));
                }
                instance.join().unwrap();
                assert_eq!(increments, if enabled { 6 } else { 0 });
            } else {
                assert_eq!(
                    instantiate_compiled(&plan, &registry)
                        .unwrap()
                        .execute_with_inputs(options().arguments)
                        .unwrap()["count"],
                    json!(2)
                );
            }
            assert_eq!(marker.exists(), enabled);
        }
    }
}

#[cfg(unix)]
#[test]
fn standalone_loop_streams_preserve_guards_delivery_and_frozen_contracts() {
    use std::{fs, process::Command};
    for enabled in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("executed");
        let mut value = guarded_definition(enabled, &marker);
        value["execution"] = json!({"limits":{"workers":1,"max_pending_messages":2}});
        value["dependencies"] = json!({
            "core":{"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")},
            "fixture":{"package":"fixture-multi-nodes","path":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes")}
        });
        let source = root.path().join("flow.json");
        let output = root.path().join("runner");
        let build = root.path().join("build");
        fs::write(&source, value.to_string()).unwrap();
        mf_compiler::compile_project_with_options(
            &mf_compiler::CompileRequest {
                definition: &source,
                output: &output,
                locked: false,
                build_dir: Some(&build),
                support: &mf_compiler::SupportPackages::Local {
                    crates_dir: common::crates_dir(),
                },
            },
            &mf_compiler::RunnerOptions { telemetry: false },
        )
        .unwrap();
        assert!(!marker.exists());
        fs::remove_file(source).unwrap();
        fs::remove_dir_all(build).unwrap();
        let finished = Command::new(&output)
            .args(["--inputs", r#"{"repeat.count":2}"#])
            .env_clear()
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(
            finished.status.success(),
            "{}",
            String::from_utf8_lossy(&finished.stderr)
        );
        let actual: Vec<Value> = serde_json::Deserializer::from_slice(&finished.stdout)
            .into_iter()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(actual.len(), if enabled { 7 } else { 1 });
        assert_eq!(
            actual
                .iter()
                .filter_map(|value| value.get("increment"))
                .cloned()
                .collect::<Vec<_>>(),
            if enabled {
                vec![json!(0), json!(1), json!(0), json!(1), json!(0), json!(1)]
            } else {
                vec![]
            }
        );
        assert_eq!(
            actual
                .iter()
                .filter(|value| value.get("count").is_some())
                .cloned()
                .collect::<Vec<_>>(),
            vec![json!({"count":2})]
        );
        assert_eq!(marker.exists(), enabled);
        if marker.exists() {
            fs::remove_file(&marker).unwrap();
        }
        let drift = Command::new(&output)
            .args(["--inputs", r#"{"repeat.count":2}"#])
            .env_clear()
            .env("PATH", "")
            .env("MF_FIXTURE_STREAM_CONTRACT_CHANGE", "1")
            .output()
            .unwrap();
        assert!(!drift.status.success());
        assert!(
            String::from_utf8_lossy(&drift.stderr).contains("prepared port contract"),
            "{}",
            String::from_utf8_lossy(&drift.stderr)
        );
        assert!(!marker.exists());
        assert!(drift.stdout.is_empty());
    }
}

#[test]
fn child_panics_close_the_active_pass_without_publishing_loop_terminal_outputs() {
    let mut value = definition();
    value["nodes"][0]["loop"]["body"]["nodes"][0]["kind"] = "fixture.task_stream_panic".into();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let harness = capture::Harness::new(true);
    let mut options = options();
    options.observation = Some(
        plan.start_stream_observation(&harness.observer(), mf_telemetry::identity::RunId::new())
            .unwrap(),
    );
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(options)
        .unwrap();
    let error = match instance.recv() {
        Ok(Some(output)) => {
            assert_eq!(output.outputs["increment"], json!(0));
            instance.recv().unwrap_err()
        }
        Err(error) => error,
        Ok(None) => panic!("a panicking child cannot complete successfully"),
    };
    assert!(error.to_string().contains("combined panic sentinel"));
    assert!(instance.join().is_err());
    let records: Vec<_> = harness
        .records()
        .iter()
        .filter(|record| record.scope == mf_telemetry::INSTRUMENTATION_SCOPE)
        .map(|record| mf_telemetry::stream::StreamRecord::decode(record).unwrap())
        .collect();
    let failed_passes = records
        .iter()
        .filter(|record| {
            matches!(
                &record.payload,
                mf_telemetry::stream::StreamPayload::Execution(
                    mf_telemetry::event::Event::LoopPassFinished {
                        outcome: mf_telemetry::event::LoopPassOutcome::Failed,
                        ..
                    }
                )
            )
        })
        .count();
    assert_eq!(failed_passes, 1);
}

#[test]
fn stream_bindings_reject_invalid_sources_names_and_contracts() {
    use mf_runtime::{FlowNode, NodeId, SubgraphStreamOutput};
    let registry = NodeRegistry::from_inventory().unwrap();
    let make_flow = || {
        let node = FlowNode::new(
            "step",
            registry
                .get("fixture.task_stream")
                .unwrap()
                .instantiate(json!({}))
                .unwrap(),
        );
        mf_compiler::FlowBuilder::prepare(vec![node], vec![], vec!["step".into()], vec![])
            .unwrap()
            .into_tasks()
            .unwrap()
    };
    let port = make_flow().nodes()[0].metadata.stream_outputs[0].clone();
    let valid = SubgraphStreamOutput {
        node: NodeId::new(0),
        port,
        name: "increment".into(),
        optional: false,
    };
    for invalid in [
        SubgraphStreamOutput {
            node: NodeId::new(2),
            ..valid.clone()
        },
        SubgraphStreamOutput {
            name: String::new(),
            ..valid.clone()
        },
        SubgraphStreamOutput {
            port: mf_runtime::PortSpec::new("delta", mf_runtime::ValueType::String, true),
            ..valid.clone()
        },
    ] {
        assert!(make_flow().with_stream_outputs(vec![invalid]).is_err());
    }
    assert!(
        make_flow()
            .with_stream_outputs(vec![valid.clone(), valid.clone()])
            .is_err()
    );
}
