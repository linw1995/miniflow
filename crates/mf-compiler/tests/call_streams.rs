mod common;
#[path = "fixtures/multi-nodes/src/task_stream.rs"]
mod fixture;
extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, compile_definition, instantiate_compiled, instantiate_stream};
use mf_runtime::{StreamOptions, WorkflowArguments};
use serde_json::{Value, json};

struct OptionalEmission;

impl mf_runtime::NodePortContract for OptionalEmission {
    fn ports(&self) -> mf_runtime::NodePorts {
        mf_runtime::NodePorts {
            inputs: vec![mf_runtime::PortSpec::new(
                "count",
                mf_runtime::ValueType::Usize,
                true,
            )],
            outputs: vec![mf_runtime::PortSpec::new(
                "total",
                mf_runtime::ValueType::Usize,
                true,
            )],
        }
    }

    fn stream_outputs(&self) -> Vec<mf_runtime::PortSpec> {
        vec![mf_runtime::PortSpec::new(
            "delta",
            mf_runtime::ValueType::Uint64,
            false,
        )]
    }
}

impl mf_runtime::TaskStreamNode for OptionalEmission {
    fn execute(
        &self,
        inputs: mf_runtime::Inputs,
        _: &mut mf_runtime::ExecutionContext,
        emitter: &mut mf_runtime::Emitter<'_>,
    ) -> Result<mf_runtime::NodeResult, mf_runtime::NodeExecutionError> {
        emitter.send(Default::default())?;
        Ok(mf_runtime::Outputs::from([("total".into(), inputs["count"].clone())]).into())
    }
}

inventory::submit! {
    mf_runtime::NodeRegistration {
        kind: "fixture.optional_call_stream",
        factory: mf_runtime::NodeFactory::Plain(|_| mf_runtime::PreparedNode::task_stream(OptionalEmission, mf_runtime::NodeMetadata::default())),
    }
}

fn child(config: Value) -> Value {
    json!({"version":"2026-10-03","dependencies":{},
        "nodes":[{"id":"step","kind":"fixture.task_stream","config":config}],
        "outputs":[{"name":"increment","node":"step","port":"delta"},
                   {"name":"count","node":"step","port":"total"}]
    })
}

fn caller(child: Value) -> Value {
    json!({"version":"2026-10-28","dependencies":{},
        "nodes":[{"id":"invoke","kind":"workflow.call","config":{"flow":"child","body":child}}],
        "outputs":[{"name":"increment","node":"invoke","port":"increment"},
                   {"name":"count","node":"invoke","port":"count"}]
    })
}

fn arguments(input: &str) -> WorkflowArguments {
    WorkflowArguments::try_from(json!({(input):2})).unwrap()
}

fn drain(value: Value, input: &str) -> Vec<Value> {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            arguments: arguments(input),
            ..Default::default()
        })
        .unwrap();
    let mut values = Vec::new();
    while let Some(output) = instance.recv().unwrap() {
        values.push(serde_json::to_value(output.outputs).unwrap());
    }
    instance.join().unwrap();
    values
}

#[test]
fn nested_calls_project_public_streams_and_keep_terminal_adaptation_unsubscribed() {
    let root = tempfile::tempdir().unwrap();
    for nested in [false, true] {
        let input = if nested {
            "invoke.invoke.step.count"
        } else {
            "invoke.step.count"
        };
        let definition = if nested {
            caller(caller(child(json!({}))))
        } else {
            caller(child(json!({})))
        };
        let values = drain(definition.clone(), input);
        assert_eq!(values.len(), 3);
        assert_eq!(
            values
                .iter()
                .filter_map(|value| value.get("increment"))
                .cloned()
                .collect::<Vec<_>>(),
            vec![json!(0), json!(1)]
        );
        assert_eq!(
            values
                .iter()
                .filter(|value| value.get("count").is_some())
                .cloned()
                .collect::<Vec<_>>(),
            vec![json!({"count":2})]
        );

        let leaf = child(json!({"wait_path":root.path().join("unreleased")}));
        let mut terminal = if nested {
            caller(caller(leaf))
        } else {
            caller(leaf)
        };
        terminal["outputs"]
            .as_array_mut()
            .unwrap()
            .retain(|output| output["name"] != "increment");
        let registry = NodeRegistry::from_inventory().unwrap();
        let plan =
            compile_definition(&serde_json::from_value(terminal).unwrap(), &registry).unwrap();
        assert_eq!(
            instantiate_compiled(&plan, &registry)
                .unwrap()
                .execute_with_inputs(arguments(input))
                .unwrap()["count"],
            json!(2)
        );
    }
}

#[test]
fn calls_export_loop_streams_without_exposing_loop_state() {
    let body = json!({"version":"2026-10-28","dependencies":{},
        "nodes":[{"id":"repeat","kind":"loop.run","config":{
            "max_iterations":2,"variables":[{"name":"count","type":"usize"}],
            "stream_outputs":{"delta":{"node":"step","port":"delta"}},
            "body":{"version":"2026-10-28", "dependencies":{}, "nodes":[{"id":"step","kind":"fixture.task_stream","inputs":{"count":{"$ref":"%loop.count"}}}]}
        }}],
        "outputs":[{"name":"increment","node":"repeat","port":"delta"}, {"name":"count","node":"repeat","port":"count"}]
    });
    let values = drain(caller(body), "invoke.repeat.count");
    assert_eq!(values.len(), 5);
    assert_eq!(
        values
            .iter()
            .filter_map(|value| value.get("increment"))
            .cloned()
            .collect::<Vec<_>>(),
        vec![json!(0), json!(1), json!(0), json!(1)]
    );
    assert_eq!(
        values
            .iter()
            .filter(|value| value.get("count").is_some())
            .cloned()
            .collect::<Vec<_>>(),
        vec![json!({"count":2})]
    );
}

#[test]
fn call_prefixes_arrive_before_completion_and_cancellation_remains_fatal() {
    let root = tempfile::tempdir().unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let value = caller(child(json!({"wait_path":root.path().join("unreleased")})));
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            arguments: arguments("invoke.step.count"),
            ..Default::default()
        })
        .unwrap();
    let first = instance.receive().unwrap().unwrap();
    assert_eq!(first.output().outputs["increment"], json!(0));
    first.acknowledge().unwrap();
    instance.cancellation().cancel(
        mf_runtime::StreamExecutionSnafu {
            message: "cancel call after prefix",
        }
        .build(),
    );
    assert!(
        instance
            .join()
            .unwrap_err()
            .to_string()
            .contains("cancel call after prefix")
    );
}

#[test]
fn public_stream_requiredness_is_preserved_independently_of_the_source_port() {
    struct Accept;
    impl mf_runtime::EmissionSink for Accept {
        fn send(&mut self, _: mf_runtime::NodeResult) -> Result<(), mf_runtime::StreamError> {
            Ok(())
        }
    }
    for optional in [false, true] {
        for multiple_sources in [false, true] {
            let mut leaf = child(json!({}));
            leaf["version"] = json!("2026-10-19");
            leaf["nodes"][0]["kind"] = json!("fixture.optional_call_stream");
            leaf["outputs"][0]["optional"] = json!(optional);
            if multiple_sources {
                leaf["nodes"].as_array_mut().unwrap().push(json!({
                    "id":"other","kind":"fixture.task_stream",
                    "inputs":{"count":{"$ref":"step.total"}}
                }));
                leaf["outputs"].as_array_mut().unwrap().push(json!({
                    "name":"other","node":"other","port":"delta","optional":true
                }));
            }
            let mut value = caller(leaf);
            value["outputs"]
                .as_array_mut()
                .unwrap()
                .retain(|output| output["name"] != "increment");
            let registry = NodeRegistry::from_inventory().unwrap();
            let plan =
                compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
            let flow = instantiate_compiled(&plan, &registry).unwrap();
            let result = flow.nodes()[0].node.execute_with_sink(
                [("step.count".into(), 2_u64.into())].into(),
                &mut mf_runtime::ExecutionContext::default(),
                &mut Accept,
            );
            assert_eq!(result.is_ok(), optional);
        }
    }
}

fn wired_child(config: Value) -> Value {
    json!({"version":"2026-10-28", "nodes":[
        {"id":"step","kind":"fixture.task_stream","config":config},
        {"id":"buffer","kind":"builtin.batch","config":{"max_items":2,"max_wait_ms":60000},
         "inputs":{"item":{"$ref":"step.delta"}}},
        {"id":"copy","kind":"builtin.identity","inputs":{"input":{"$ref":"buffer.items"}}}
    ], "outputs":[{"name":"increment","node":"copy","port":"value"},{"name":"count","node":"step","port":"total"}]})
}

#[test]
fn callback_bindings_replace_a_scheduled_body_only_when_its_public_interface_is_finite() {
    let mut registry = NodeRegistry::from_inventory().unwrap();
    let mut document = caller(wired_child(json!({})));
    let config = &mut document["nodes"][0]["config"];
    config["flow"] = json!("callback:count");
    let parse = |document: &Value| serde_json::from_value(document.clone()).unwrap();
    let error = compile_definition(&parse(&document), &registry).unwrap_err();
    assert!(error.to_string().contains("finite interface"), "{error}");

    document["nodes"][0]["config"]["body"]["outputs"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    document["outputs"].as_array_mut().unwrap().remove(0);
    let plan = compile_definition(&parse(&document), &registry).unwrap();
    let run = |registry: &NodeRegistry| {
        mf_compiler::execute_compiled_with_inputs(
            &plan,
            registry,
            arguments("invoke.step.count"),
            None,
        )
        .unwrap()["count"]
            .clone()
    };
    assert_eq!(run(&registry), json!(2));
    let sites = plan.manifest(&registry).unwrap().descriptor.callbacks;
    let callback = mf_runtime::callback::HostCallback::new(|args, _| {
        assert_eq!(args.0["step.count"], json!(2));
        Ok(mf_runtime::Outputs::from([("count".into(), 17_u64.into())]).into())
    });
    registry
        .bind_callbacks(&sites, &[], &[("count".into(), callback)].into())
        .unwrap();
    assert_eq!(run(&registry), json!(17));
}

#[test]
fn callable_stream_edges_flush_before_return_and_isolate_concurrent_invocations() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(
        &serde_json::from_value(caller(wired_child(json!({})))).unwrap(),
        &registry,
    )
    .unwrap();
    let body = mf_compiler::prepare_call_body(&plan, &registry, "invoke").unwrap();
    let flow = mf_runtime::FlowValue::new(body).unwrap();
    assert_eq!(flow.stream_output_ports()[0].name, "increment");
    assert_eq!(flow.output_ports()[0].name, "count");
    std::thread::scope(|threads| {
        for count in [5_usize, 0, 4, 1] {
            let flow = &flow;
            threads.spawn(move || {
                let mut ctx = mf_runtime::ExecutionContext::default();
                ctx.configure_worker_limit(1.try_into().unwrap());
                let args = json!({"step.count":count}).try_into().unwrap();
                ctx.set_workflow_arguments(args);
                let invocation = flow.body().start_in_context(ctx).unwrap();
                let mut batches = Vec::new();
                while let Some(record) = invocation.recv().unwrap() {
                    assert_eq!(record.result.outputs.len(), 1);
                    batches
                        .push(serde_json::to_value(&record.result.outputs["increment"]).unwrap());
                }
                assert_eq!(
                    batches,
                    (0..count)
                        .collect::<Vec<_>>()
                        .chunks(2)
                        .map(|b| json!(b))
                        .collect::<Vec<_>>()
                );
                assert_eq!(invocation.join().unwrap().unwrap()["count"], json!(count));
            });
        }
    });
}

struct Receiver<F>(F);
impl<F: FnMut(mf_runtime::NodeResult) -> Result<(), mf_runtime::StreamError>>
    mf_runtime::EmissionSink for Receiver<F>
{
    fn send(&mut self, packet: mf_runtime::NodeResult) -> Result<(), mf_runtime::StreamError> {
        (self.0)(packet)
    }
}

fn callable(child: Value) -> mf_runtime::FlowValue {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan =
        compile_definition(&serde_json::from_value(caller(child)).unwrap(), &registry).unwrap();
    mf_runtime::FlowValue::new(mf_compiler::prepare_call_body(&plan, &registry, "invoke").unwrap())
        .unwrap()
}

#[test]
fn callable_message_selections_keep_related_values_together_and_skip_absent_domains() {
    let mut leaf = wired_child(json!({}));
    let outputs = leaf["outputs"].as_array_mut().unwrap();
    outputs.push(json!({"name":"original","node":"buffer","port":"items"}));
    outputs.push(json!({"name":"delta","node":"step","port":"delta"}));
    let flow = callable(leaf);
    assert!(flow.stream_output_ports().iter().all(|p| !p.required));
    let mut ctx = mf_runtime::ExecutionContext::default();
    let mut deltas = Vec::new();
    let mut batches = Vec::new();
    let result = flow
        .invoke_with_sink(
            "caller",
            json!({"step.count":3}).try_into().unwrap(),
            &mut ctx,
            &mut Receiver(|record: mf_runtime::NodeResult| {
                if let Some(delta) = record.outputs.get("delta") {
                    assert_eq!(
                        record.skipped,
                        ["increment".to_string(), "original".to_string()].into()
                    );
                    deltas.push(delta.as_u64().unwrap());
                } else {
                    assert_eq!(record.skipped, ["delta".to_string()].into());
                    assert!(record.outputs["increment"].ptr_eq(&record.outputs["original"]));
                    batches.push(serde_json::to_value(&record.outputs["increment"]).unwrap());
                }
                Ok(())
            }),
        )
        .unwrap();
    assert_eq!(result["count"], json!(3));
    assert_eq!(deltas, [0, 1, 2]);
    assert_eq!(batches, [json!([0, 1]), json!([2])]);
}

#[test]
fn a_task_stream_edge_alone_schedules_a_callable_body_and_preserves_cancellation() {
    let root = tempfile::tempdir().unwrap();
    let mut leaf = child(json!({"wait_path":root.path().join("unreleased")}));
    leaf["version"] = json!("2026-10-28");
    leaf["nodes"].as_array_mut().unwrap().push(
        json!({"id":"copy","kind":"builtin.identity","inputs":{"input":{"$ref":"step.delta"}}}),
    );
    leaf["outputs"][0] = json!({"name":"increment","node":"copy","port":"value"});
    let flow = callable(leaf);
    let mut ctx = mf_runtime::ExecutionContext::default();
    let token = ctx.cancellation();
    let mut delivered = 0;
    let result = flow.invoke_with_sink(
        "caller",
        arguments("step.count"),
        &mut ctx,
        &mut Receiver(|packet: mf_runtime::NodeResult| {
            assert_eq!(packet.outputs["increment"], json!(0));
            delivered += 1;
            token.cancel(
                mf_runtime::StreamExecutionSnafu {
                    message: "stop child after prefix",
                }
                .build(),
            );
            Ok(())
        }),
    );
    assert_eq!(delivered, 1);
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("stop child after prefix")
    );
}

#[test]
fn reconstructed_operators_must_still_match_their_prepared_contract() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static DRIFT: AtomicUsize = AtomicUsize::new(0);
    inventory::submit! { mf_runtime::NodeRegistration { kind: "fixture.wired_operator", factory: mf_runtime::NodeFactory::Plain(|config| {
        let registry = NodeRegistry::from_inventory().unwrap();
        let drift = DRIFT.load(Ordering::Relaxed);
        let kind = if drift == 1 { "builtin.identity" } else { "builtin.batch" };
        let mut node = registry.get(kind).unwrap().instantiate(config)?;
        if drift == 2 { node.metadata.ports.outputs.clear(); }
        Ok(node)
    }) } }
    let mut leaf = wired_child(json!({}));
    leaf["nodes"][1]["kind"] = json!("fixture.wired_operator");
    let flow = callable(leaf);
    let mut ctx = mf_runtime::ExecutionContext::default();
    for (drift, reason) in [(1, "execution kind"), (2, "port contract")] {
        DRIFT.store(drift, Ordering::Relaxed);
        let error = flow
            .invoke("caller", arguments("step.count"), &mut ctx)
            .unwrap_err();
        assert!(error.to_string().contains(reason), "{error}");
        assert!(ctx.cancellation().failure().is_none());
    }
    DRIFT.store(0, Ordering::Relaxed);
    assert_eq!(
        flow.invoke("recovered", arguments("step.count"), &mut ctx)
            .unwrap()["count"],
        json!(2)
    );
}

#[test]
fn pure_producer_runs_inside_a_call_and_a_rejected_delivery_cancels_its_receiver() {
    let flow = callable(json!({"version":"2026-10-28","nodes":[
        {"id":"feed","kind":"fixture.task_stream_feed","config":{"counts":[1,2,3]}},
        {"id":"copy","kind":"builtin.identity","inputs":{"input":{"$ref":"feed.count"}}},
        {"id":"startup","kind":"builtin.constant","config":{"value":42}}
    ],"outputs":[{"name":"increment","node":"copy","port":"value"},{"name":"count","node":"startup","port":"value"}]}));
    let mut ctx = mf_runtime::ExecutionContext::default();
    let mut seen = Vec::new();
    let result = flow
        .invoke_with_sink(
            "caller",
            Default::default(),
            &mut ctx,
            &mut Receiver(|record: mf_runtime::NodeResult| {
                seen.push(record.outputs["increment"].as_u64().unwrap());
                if seen.len() == 2 {
                    return Err(mf_runtime::StreamExecutionSnafu {
                        message: "receiver closed",
                    }
                    .build());
                }
                Ok(())
            }),
        )
        .unwrap_err();
    assert_eq!(seen, [1, 2]);
    assert!(result.to_string().contains("receiver closed"));
    assert!(ctx.cancellation().failure().is_some());
    let mut fresh = mf_runtime::ExecutionContext::default();
    assert_eq!(
        flow.invoke("again", Default::default(), &mut fresh)
            .unwrap()["count"],
        json!(42)
    );
}

#[test]
fn child_streams_reject_cross_domain_joins_and_fail_before_returning_the_startup_frame() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut invalid = wired_child(json!({}));
    invalid["nodes"].as_array_mut().unwrap().push(
        json!({"id":"join","kind":"fixture.task_stream_join", "inputs":{
        "delta":{"$ref":"step.delta"}, "total":{"$ref":"step.total"}}}),
    );
    let error = compile_definition(&serde_json::from_value(caller(invalid)).unwrap(), &registry)
        .unwrap_err();
    assert!(error.to_string().contains("one message domain"), "{error}");

    let mut leaf = wired_child(json!({}));
    // A downstream failure must suppress even an independently selected startup result.
    leaf["nodes"].as_array_mut().unwrap().push(json!({"id":"fail","kind":"fixture.iteration_stream", "config":{"fail_count":0}, "inputs":{"count":{"$ref":"step.delta"}}}));
    let mut parent = caller(leaf);
    parent["outputs"]
        .as_array_mut()
        .unwrap()
        .retain(|o| o["name"] == "count");
    let plan = compile_definition(&serde_json::from_value(parent).unwrap(), &registry).unwrap();
    let error = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_with_inputs(arguments("invoke.step.count"))
        .unwrap_err();
    assert!(error.to_string().contains("item 0 failed"), "{error}");
}

#[cfg(unix)]
#[test]
fn standalone_calls_forward_frozen_streams_without_sources() {
    use std::{fs, process::Command};
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("executed");
    let source = root.path().join("parent.json");
    let child_path = root.path().join("child.json");
    let output = root.path().join("runner");
    let build = root.path().join("build");
    let mut leaf = wired_child(json!({"marker":marker}));
    leaf["execution"] = json!({});
    leaf["dependencies"] = json!({"fixture":{"package":"fixture-multi-nodes","path":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes")}});
    let mut parent = caller(leaf.clone());
    parent["nodes"][0]["config"]
        .as_object_mut()
        .unwrap()
        .remove("body");
    parent["flows"] = json!({"child":{"path":"child.json"}});
    parent["dependencies"] = json!({"core":{"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")}});
    fs::write(&source, parent.to_string()).unwrap();
    fs::write(&child_path, leaf.to_string()).unwrap();
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
    fs::remove_file(child_path).unwrap();
    fs::remove_dir_all(build).unwrap();
    let invoke = |drift: bool| {
        let mut command = Command::new(&output);
        command
            .args(["--inputs", r#"{"invoke.step.count":3}"#])
            .env_clear()
            .env("PATH", "");
        if drift {
            command.env("MF_FIXTURE_STREAM_CONTRACT_CHANGE", "1");
        }
        command.output().unwrap()
    };
    let drift = invoke(true);
    assert!(!drift.status.success());
    assert!(String::from_utf8_lossy(&drift.stderr).contains("prepared port contract"));
    assert!(drift.stdout.is_empty());
    assert!(!marker.exists());
    let result = invoke(false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(marker.exists());
    let values: Vec<Value> = serde_json::Deserializer::from_slice(&result.stdout)
        .into_iter()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(values.len(), 3);
    assert_eq!(
        values
            .iter()
            .filter_map(|value| value.get("increment"))
            .cloned()
            .collect::<Vec<_>>(),
        vec![json!([0, 1]), json!([2])]
    );
    assert_eq!(
        values
            .iter()
            .filter(|value| value.get("count").is_some())
            .cloned()
            .collect::<Vec<_>>(),
        vec![json!({"count":3})]
    );
}

#[test]
fn declared_child_execution_limits_are_validated_before_calling() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for limit in ["workers", "max_pending_messages"] {
        let mut leaf = child(json!({}));
        leaf["execution"] = json!({"limits":{(limit):0}});
        let error = compile_definition(&serde_json::from_value(caller(leaf)).unwrap(), &registry)
            .unwrap_err();
        assert!(error.to_string().contains("stream limits must be positive"));
    }
}

#[test]
fn calls_forward_parallel_iteration_streams_and_keep_collected_results() {
    let body = json!({"version":"2026-10-28","dependencies":{},
        "nodes":[
            {"id":"items","kind":"builtin.constant","config":{"value":[1,2]}},
            {"id":"each","kind":"builtin.iteration","inputs":{"items":{"$ref":"items.value"}},
             "config":{"mode":"parallel","stream_outputs":{"result":{"node":"step","port":"delta"}},
                "body":{"version":"2026-10-28", "dependencies":{}, "nodes":[{"id":"step","kind":"fixture.iteration_stream","inputs":{"count":{"$ref":"%iteration.item"}}}],
                        "outputs": [{"name":"result", "node":"step","port":"total"}]}}}
        ],
        "outputs":[{"name":"increment","node":"each","port":"result"},{"name":"count","node":"each","port":"results"}]
    });
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = caller(body);
    value["execution"] = json!({"limits":{"workers":2}});
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start()
        .unwrap();
    let mut values = Vec::new();
    while let Some(output) = instance.recv().unwrap() {
        values.push(serde_json::to_value(output.outputs).unwrap());
    }
    instance.join().unwrap();
    let mut increments: Vec<_> = values
        .iter()
        .filter_map(|value| value.get("increment"))
        .cloned()
        .collect();
    increments.sort_by_key(|value| value.as_u64().unwrap());
    assert_eq!(increments, vec![json!(1), json!(2)]);
    assert_eq!(
        values
            .iter()
            .filter_map(|value| value.get("count"))
            .cloned()
            .collect::<Vec<_>>(),
        vec![json!([1, 2])]
    );
}
