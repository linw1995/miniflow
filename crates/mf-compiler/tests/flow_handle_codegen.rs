#![cfg(unix)]

mod common;

#[cfg(feature = "codegen")]
#[path = "fixtures/multi-nodes/src/event_flow.rs"]
mod event_fixture;
#[cfg(feature = "codegen")]
#[path = "fixtures/multi-nodes/src/task_stream.rs"]
mod stream_fixture;
extern crate mfn_core as _;

use serde_json::{Value, json};

#[cfg(feature = "codegen")]
mod in_process {
    use super::*;
    use mf_compiler::{
        CompiledWorkflow, GeneratedExecutionArtifacts, NodeRegistry, WorkflowDefinition,
        compile_definition,
    };
    use mf_runtime::{NodeTarget, PortSpec, ValueType, WorkflowManifest};

    fn finite_body() -> Value {
        json!({
            "version":"2026-11-01", "dependencies":{},
            "nodes":[{"id":"step", "kind":"fixture.task_stream"}],
            "outputs":[{"name":"increment", "node":"step", "port":"delta"},
                       {"name":"count", "node":"step", "port":"total"}]
        })
    }

    fn event_body() -> Value {
        json!({
            "version":"2026-11-01", "dependencies":{},
            "nodes":[
                {"id":"step", "kind":"fixture.task_stream"},
                {"id":"batch", "kind":"fixture.flow_batch",
                 "inputs":{"item":{"from":{"node":"step", "port":"delta"}}}},
                {"id":"copy", "kind":"builtin.identity",
                 "inputs":{"input":{"from":{"node":"batch", "port":"items"}}}}
            ],
            "outputs":[{"name":"batch", "node":"copy", "port":"value"}]
        })
    }

    fn static_call(body: Value, outputs: Value) -> Value {
        json!({
            "version":"2026-11-01", "dependencies":{},
            "nodes":[{"id":"invoke", "kind":"workflow.call", "config":{
                "flow":"child", "definition":body
            }}],
            "outputs":outputs
        })
    }

    fn passed_call(body: Value, outputs: Value) -> Value {
        json!({
            "version":"2026-11-01", "dependencies":{},
            "flow_arguments":{"child":{"definition":body, "description":null, "parameters":{}}},
            "nodes":[
                {"id":"select", "kind":"builtin.constant", "config":{"value":"child"}},
                {"id":"args", "kind":"builtin.constant", "config":{"value":{"step.count":2}}},
                {"id":"invoke", "kind":"workflow.call", "config":{
                    "flow_ref":{"$ref":"select.value"}, "arguments_object":true, "capture_errors":true
                }, "inputs":{"arguments":{"from":{"node":"args", "port":"value"}}}}
            ],
            "edges":[{"from_node":"select", "to_node":"invoke"}],
            "outputs":outputs
        })
    }

    // These are the build-time APIs: exercise them in the instrumented test process,
    // not in the build script of a separately compiled standalone runner.
    fn generate(value: Value) -> (CompiledWorkflow, GeneratedExecutionArtifacts) {
        let registry = NodeRegistry::from_inventory().unwrap();
        let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let layouts = plan.generate_execution_plans(&registry).unwrap();
        syn::parse_file(&layouts.rust_source).unwrap();
        let manifest = WorkflowManifest::from_bytes(&layouts.manifest_bytes).unwrap();
        assert_eq!(manifest.definition, plan.definition);
        assert_eq!(manifest, plan.manifest(&registry).unwrap());
        assert_eq!(
            compile_definition(&manifest.definition, &registry).unwrap(),
            plan
        );
        let targets: Vec<NodeTarget> = serde_json::from_str(&layouts.node_targets_json).unwrap();
        assert_eq!(targets.len(), plan.execution_order.len());
        for (index, target) in targets.iter().enumerate() {
            assert_eq!(target.index, index);
            assert_eq!(target.id, plan.execution_order[index].as_str());
            target.description.validate().unwrap();
        }
        let runner_layouts = plan.generate_runner_execution_plans(&registry).unwrap();
        syn::parse_file(&runner_layouts.rust_source).unwrap();
        assert_eq!(runner_layouts.manifest_bytes, layouts.manifest_bytes);
        assert_eq!(runner_layouts.node_targets_json, layouts.node_targets_json);
        for artifacts in [
            plan.generate_artifacts().unwrap(),
            plan.generate_runner_artifacts().unwrap(),
        ] {
            syn::parse_file(&artifacts.rust_source).unwrap();
            assert_eq!(
                CompiledWorkflow::from_json(&artifacts.plan_json).unwrap(),
                plan
            );
            assert_no_runtime_interpretation(&artifacts.rust_source);
        }
        assert_no_runtime_interpretation(&layouts.rust_source);
        assert_no_runtime_interpretation(&runner_layouts.rust_source);
        (plan, layouts)
    }

    fn assert_no_runtime_interpretation(source: &str) {
        for forbidden in [
            "CompiledWorkflow::from_json",
            "WorkflowDefinition::from_json",
            "compile_definition",
            "resolve_flows",
            "instantiate_compiled",
            "instantiate_stream",
            "prepare_stream_body",
            "outputs_for_domain",
            "plan.dependencies(",
        ] {
            assert!(
                !source.contains(forbidden),
                "runtime interpreter: {forbidden}"
            );
        }
    }

    fn function<'a>(items: &'a [syn::Item], name: &str) -> &'a syn::ItemFn {
        items
            .iter()
            .find_map(|item| match item {
                syn::Item::Fn(function) if function.sig.ident == name => Some(function),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing generated function {name}"))
    }

    fn stream_modules(items: &[syn::Item]) -> Vec<&syn::ItemMod> {
        items
            .iter()
            .filter_map(|item| match item {
                syn::Item::Mod(module) if module.ident.to_string().starts_with("stream_body_") => {
                    Some(module)
                }
                _ => None,
            })
            .collect()
    }

    fn stream_ports(module: &syn::ItemMod) -> Vec<PortSpec> {
        let items = &module.content.as_ref().unwrap().1;
        for name in [
            "prepare",
            "stream_program",
            "verify_stream_metadata",
            "bind_stream_guards",
        ] {
            function(items, name);
        }
        // Read the actual serialized PortSpecs, without depending on Rust whitespace
        // or punctuation chosen by prettyplease.
        let output_ports = function(items, "output_ports");
        let Some(syn::Stmt::Expr(syn::Expr::MethodCall(context), None)) =
            output_ports.block.stmts.last()
        else {
            panic!("output_ports must return a contextualized metadata decode")
        };
        let syn::Expr::Call(decode) = context.receiver.as_ref() else {
            panic!("output_ports must decode frozen metadata")
        };
        let Some(syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(json),
            ..
        })) = decode.args.first()
        else {
            panic!("output_ports must contain frozen JSON")
        };
        serde_json::from_str(&json.value()).unwrap()
    }

    #[test]
    fn in_process_root_stream_preparation_preserves_incremental_and_terminal_contracts() {
        // No data bindings exercises the empty input-binding closure too.
        let (plan, layouts) = generate(finite_body());
        assert!(plan.definition.execution.is_some());
        let artifacts = plan.generate_artifacts().unwrap();
        let syntax = syn::parse_file(&artifacts.rust_source).unwrap();
        function(&syntax.items, "prepare_stream");
        let program = syn::parse_file(&layouts.rust_source).unwrap();
        function(&program.items, "stream_program");
        function(&program.items, "select_stream_outputs_0");
        function(&program.items, "initialize_stream_message_0");
        assert!(stream_modules(&program.items).is_empty());
        let targets: Vec<NodeTarget> = serde_json::from_str(&layouts.node_targets_json).unwrap();
        assert_eq!(
            targets[0]
                .outputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["total"]
        );
        let manifest = WorkflowManifest::from_bytes(&layouts.manifest_bytes).unwrap();
        assert_eq!(
            manifest.input_schema.inputs["step"]["count"].value_type,
            ValueType::Usize
        );
    }

    #[test]
    fn in_process_static_event_child_freezes_a_required_single_domain_export() {
        let value = static_call(
            event_body(),
            json!([
                {"name":"batch", "node":"invoke", "port":"batch"}
            ]),
        );
        let (plan, layouts) = generate(value);
        let body: WorkflowDefinition =
            serde_json::from_value(plan.definition.nodes[0].config["definition"].clone()).unwrap();
        assert!(body.execution.is_some());
        let syntax = syn::parse_file(&layouts.rust_source).unwrap();
        let modules = stream_modules(&syntax.items);
        assert_eq!(modules.len(), 1);
        assert_eq!(
            stream_ports(modules[0]),
            [PortSpec::new("batch", ValueType::Array, true)]
        );
        let manifest = WorkflowManifest::from_bytes(&layouts.manifest_bytes).unwrap();
        assert_eq!(
            manifest.input_schema.inputs["invoke"]["step.count"].value_type,
            ValueType::Usize
        );
        let artifacts = plan.generate_artifacts().unwrap();
        assert!(
            artifacts
                .rust_source
                .contains("PreparedSubgraph::from_stream_factory")
        );
        assert!(layouts.rust_source.contains("verify_generated_metadata"));
    }

    #[test]
    fn in_process_nested_static_and_passed_event_bodies_keep_distinct_programs() {
        let selected = json!([
            {"name":"batch", "node":"relay", "port":"value"},
            {"name":"completion", "node":"invoke", "port":"result"}
        ]);
        let mut inner = passed_call(event_body(), selected);
        inner["nodes"].as_array_mut().unwrap().push(json!({
            "id":"relay", "kind":"builtin.identity",
            "inputs":{"input":{"from":{"node":"invoke", "port":"batch"}}}
        }));
        let outer = static_call(
            inner,
            json!([
                {"name":"batch", "node":"invoke", "port":"batch"},
                {"name":"completion", "node":"invoke", "port":"completion"}
            ]),
        );
        let (_, layouts) = generate(outer);
        let syntax = syn::parse_file(&layouts.rust_source).unwrap();
        let modules = stream_modules(&syntax.items);
        // The outer static child owns the receiver's stream routing; the passed
        // Event body owns a separate operator instance and metadata contract.
        assert_eq!(modules.len(), 2);
        let ports: Vec<_> = modules.iter().map(|module| stream_ports(module)).collect();
        assert!(
            ports
                .iter()
                .any(|ports| ports == &[PortSpec::new("batch", ValueType::Array, true)])
        );
        assert!(ports.iter().any(|ports| ports.len() == 2
            && ports.iter().all(|port| !port.required)
            && ports.iter().any(|port| port.name == "completion")));
        assert!(layouts.rust_source.contains("PreparedFlowArguments::new"));
        assert!(
            layouts
                .rust_source
                .contains("PreparedSubgraph::from_stream_factory")
        );
    }

    #[test]
    fn in_process_event_exports_are_sparse_across_startup_and_event_domains() {
        let mut body = event_body();
        body["outputs"].as_array_mut().unwrap().extend([
            json!({"name":"count", "node":"step", "port":"total"}),
            json!({"name":"increment", "node":"step", "port":"delta", "optional":true}),
        ]);
        let (_, layouts) = generate(static_call(body, json!([])));
        let syntax = syn::parse_file(&layouts.rust_source).unwrap();
        let modules = stream_modules(&syntax.items);
        assert_eq!(modules.len(), 1);
        let ports = stream_ports(modules[0]);
        assert_eq!(
            ports,
            [
                PortSpec::new("batch", ValueType::Array, false),
                PortSpec::new("count", ValueType::Usize, false),
                PortSpec::new("increment", ValueType::Usize, false),
            ]
        );
        let items = &modules[0].content.as_ref().unwrap().1;
        let selectors = items.iter().filter(|item| matches!(item,
            syn::Item::Fn(function) if function.sig.ident.to_string().starts_with("select_stream_outputs_")
        )).count();
        assert_eq!(
            selectors, 3,
            "each independent output domain needs its own selector"
        );
    }

    #[test]
    fn in_process_passed_finite_bindings_keep_stream_exports_out_of_terminal_plans() {
        let mut body = finite_body();
        body["outputs"][0]["optional"] = json!(true);
        let mut value = passed_call(
            body.clone(),
            json!([
                {"name":"increment", "node":"invoke", "port":"increment", "optional":true},
                {"name":"completion", "node":"invoke", "port":"result"}
            ]),
        );
        // This alternate returns only a terminal count, so the shared incremental
        // port must remain optional even when selecting another frozen binding.
        body["outputs"] = json!([{"name":"count", "node":"step", "port":"total"}]);
        value["flow_arguments"]["alternate"] = json!({
            "definition":body, "description":null, "parameters":{}
        });
        let (plan, layouts) = generate(value);
        for argument in plan.definition.flow_arguments.values() {
            assert!(
                argument.definition.execution.is_none(),
                "finite bodies must retain the Task fast path"
            );
        }
        let syntax = syn::parse_file(&layouts.rust_source).unwrap();
        assert!(stream_modules(&syntax.items).is_empty());
        let body_plans: Vec<_> = syntax
            .items
            .iter()
            .filter_map(|item| match item {
                syn::Item::Static(item) if item.ident.to_string().contains("_BINDING_") => {
                    Some(item)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            body_plans.len(),
            2,
            "both frozen bindings need a Task layout"
        );
        for body_plan in body_plans {
            let syn::Expr::Struct(layout) = body_plan.expr.as_ref() else {
                panic!("Task layout must be a static FlowPlan")
            };
            let outputs = layout
                .fields
                .iter()
                .find(|field| {
                    matches!(
                        &field.member, syn::Member::Named(name) if name == "outputs"
                    )
                })
                .unwrap();
            let source = quote::quote!(#outputs).to_string();
            assert!(source.contains("count"));
            assert!(
                !source.contains("increment"),
                "incremental values are not terminal outputs"
            );
        }
        let artifacts = plan.generate_artifacts().unwrap();
        assert!(
            artifacts
                .rust_source
                .contains("PreparedSubgraph::from_flow")
        );
        assert!(artifacts.rust_source.contains("PreparedFlowArguments::new"));
        assert!(layouts.rust_source.contains("SubgraphStreamOutput"));
    }
}

#[test]
fn passed_finite_task_stream_keeps_incremental_and_terminal_exports_separate() {
    use std::{fs, process::Command};
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("flow.json");
    let output = root.path().join("runner");
    let build = root.path().join("build");
    let body = json!({
        "version":"2026-11-01", "dependencies":{},
        "nodes":[{"id":"step", "kind":"fixture.task_stream"}],
        "outputs":[{"name":"increment", "node":"step", "port":"delta"},
                   {"name":"count", "node":"step", "port":"total"}]
    });
    let value = json!({
        "version":"2026-11-01",
        "dependencies":{
            "core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")},
            "fixture":{"package":"fixture-multi-nodes", "path":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes")}
        },
        "flow_arguments":{"child":{"definition":body,"description":null,"parameters":{}}},
        "nodes":[
            {"id":"select", "kind":"builtin.constant", "config":{"value":"child"}},
            {"id":"args", "kind":"builtin.constant", "config":{"value":{"step.count":2}}},
            {"id":"invoke", "kind":"workflow.call", "config":{
                "flow_ref":{"$ref":"select.value"}, "arguments_object":true, "capture_errors":true
            }, "inputs":{"arguments":{"from":{"node":"args", "port":"value"}}}}
        ],
        "edges":[{"from_node":"select", "to_node":"invoke"}],
        "outputs":[{"name":"increment", "node":"invoke", "port":"increment"},
                   {"name":"completion", "node":"invoke", "port":"result"}]
    });
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
    fs::remove_file(source).unwrap();
    fs::remove_dir_all(build).unwrap();
    let result = Command::new(output)
        .env_clear()
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let values: Vec<Value> = serde_json::Deserializer::from_slice(&result.stdout)
        .into_iter()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(values.len(), 3);
    assert!(values.contains(&json!({"increment":0})));
    assert!(values.contains(&json!({"increment":1})));
    assert!(values.contains(&json!({"completion":{"outputs":{"count":2}}})));
}
