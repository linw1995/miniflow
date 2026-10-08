mod common;
#[path = "fixtures/multi-nodes/src/struct_inputs.rs"]
mod fixture;
extern crate mfn_core as _;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, describe_workflow_inputs,
    instantiate_compiled,
};
use mf_runtime::{ValueType, WorkflowArguments};
use serde_json::{Value, json};

fn definition(config: Value) -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version":"2026-10-03",
        "dependencies":{"fixture":{"package":"fixture-multi-nodes", "path":common::crates_dir().join("mf-compiler/tests/fixtures/multi-nodes")}},
        "nodes":[{"id":"typed./~", "kind":"fixture.struct_inputs", "config":config}],
        "outputs":[{"name":"result", "node":"typed./~", "port":"value"}]
    })).unwrap()
}

fn arguments() -> Value {
    json!({"typed./~":{
        "count":7, "ratio":1.5, "active":true,
        "rows./~":[{"count":1},{"count":2}]
    }})
}

type FourLevels = Vec<Vec<Vec<Vec<mf_runtime::ValueRef>>>>;
type EightLevels = Vec<Vec<Vec<Vec<FourLevels>>>>;
type SixteenLevels = Vec<Vec<Vec<Vec<Vec<Vec<Vec<Vec<EightLevels>>>>>>>>;

#[derive(mf_runtime::NodeInputs, mf_runtime::NodeOutputs)]
struct DeepInputs {
    items: SixteenLevels,
}

#[derive(mf_runtime::NodeInputs, mf_runtime::NodeOutputs)]
struct DeepOutputs {}

struct DeepTask;
impl mf_runtime::TypedTaskNode for DeepTask {
    type Input = DeepInputs;
    type Output = DeepOutputs;

    fn execute(
        &self,
        input: Self::Input,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
        let _ = input.items;
        panic!("excessive derived depth must fail before execution");
    }
}

inventory::submit! { mf_runtime::NodeRegistration {
    kind: "test.deep_struct", factory: mf_runtime::NodeFactory::Plain(|_| {
        mf_runtime::PreparedNode::typed_task(DeepTask, mf_runtime::NodePorts::default())
    })
} }

struct DeepOutputTask;
impl mf_runtime::TypedTaskNode for DeepOutputTask {
    type Input = DeepOutputs;
    type Output = DeepInputs;

    fn execute(
        &self,
        _: Self::Input,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
        panic!("excessive derived depth must fail before execution");
    }
}

inventory::submit! { mf_runtime::NodeRegistration {
    kind: "test.deep_output_struct", factory: mf_runtime::NodeFactory::Plain(|_| {
        mf_runtime::PreparedNode::typed_task(DeepOutputTask, mf_runtime::NodePorts::default())
    })
} }

#[test]
fn compiler_rejects_excessive_derived_descriptor_depth() {
    use std::error::Error;

    for kind in ["test.deep_struct", "test.deep_output_struct"] {
        let definition: WorkflowDefinition = serde_json::from_value(json!({
            "version":"2026-10-03", "dependencies":{},
            "nodes":[{"id":"deep", "kind":kind}], "outputs":[]
        }))
        .unwrap();
        let error =
            compile_definition(&definition, &NodeRegistry::from_inventory().unwrap()).unwrap_err();
        assert!(error.to_string().contains("items"));
        let depth = error
            .source()
            .unwrap()
            .downcast_ref::<mf_runtime::TypeDepthError>()
            .unwrap();
        assert_eq!(depth.depth, ValueType::MAX_DEPTH + 1);
    }
}

#[test]
fn struct_inputs_define_startup_contracts_and_validate_before_business_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("executed");
    let definition = definition(json!({"marker":marker}));
    let registry = NodeRegistry::from_inventory().unwrap();
    let compiled = compile_definition(&definition, &registry).unwrap();
    let schema = describe_workflow_inputs(&compiled, &registry).unwrap();
    let ports = &schema.inputs["typed./~"];
    assert_eq!(ports["count"].value_type, ValueType::Int64);
    assert!(ports["count"].required);
    assert_eq!(ports["ratio"].value_type, ValueType::Float64);
    assert_eq!(ports["active"].value_type, ValueType::Boolean);
    assert_eq!(
        ports["rows./~"].value_type,
        ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Int64))))
    );
    assert_eq!(ports["label"].value_type, ValueType::String);
    assert!(!ports["label"].required && !ports["raw"].required);
    assert!(!marker.exists());
    let flow = instantiate_compiled(&compiled, &registry).unwrap();
    for (port, value, pointer) in [
        ("count", json!("wrong"), "/typed.~1~0/count"),
        ("ratio", json!(1), "/typed.~1~0/ratio"),
        ("label", json!(null), "/typed.~1~0/label"),
        (
            "rows./~",
            json!([{"count":1},{"count":"wrong"}]),
            "/typed.~1~0/rows.~1~0/1/count",
        ),
        ("unknown", json!(true), "/typed.~1~0/unknown"),
    ] {
        let mut invalid = arguments();
        invalid["typed./~"][port] = value;
        let error = flow
            .execute_with_inputs(WorkflowArguments::try_from(invalid).unwrap())
            .unwrap_err();
        assert!(error.to_string().contains(pointer), "{error}");
        assert!(!marker.exists());
    }
    let mut missing = arguments();
    missing["typed./~"].as_object_mut().unwrap().remove("count");
    assert!(
        flow.execute_with_inputs(WorkflowArguments::try_from(missing).unwrap())
            .is_err()
    );
    assert!(!marker.exists());

    let outputs = flow
        .execute_with_inputs(WorkflowArguments::try_from(arguments()).unwrap())
        .unwrap();
    assert_eq!(
        outputs["result"],
        json!({"count":7,"ratio":1.5,"active":true,"rows":[{"count":1},{"count":2}],"label":null,"raw":null,"raw_present":false})
    );
    assert_eq!(std::fs::read(&marker).unwrap(), b"executed");
    let mut supplied = arguments();
    supplied["typed./~"]["raw"] = json!(null);
    supplied["typed./~"]["label"] = json!("provided");
    let outputs = flow
        .execute_with_inputs(WorkflowArguments::try_from(supplied).unwrap())
        .unwrap();
    assert_eq!(outputs["result"]["raw_present"], json!(true));
    assert_eq!(outputs["result"]["label"], json!("provided"));
}

#[test]
fn ordinary_compiler_validation_rejects_disjoint_derived_inputs() {
    let mut value = serde_json::to_value(definition(json!({}))).unwrap();
    let mut edges = Vec::new();
    for (index, (port, input)) in [
        ("count", json!("wrong")),
        ("ratio", json!(1.5)),
        ("active", json!(true)),
        ("rows./~", json!([])),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("source_{index}");
        value["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":id,"kind":"builtin.constant","config":{"value":input}}));
        edges.push(
            json!({"from_node":id,"from_output":"value","to_node":"typed./~","to_input":port}),
        );
    }
    value["edges"] = json!(edges);
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let error =
        compile_definition(&definition, &NodeRegistry::from_inventory().unwrap()).unwrap_err();
    let diagnostic = error.to_string();
    assert!(
        diagnostic.contains("count") && diagnostic.contains("int64"),
        "{diagnostic}"
    );
}

#[cfg(all(unix, feature = "codegen"))]
#[test]
fn generated_struct_inputs_match_memory_and_freeze_the_interface() {
    use mf_compiler::{
        CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options,
    };
    use std::{
        fs,
        process::{Command, Stdio},
    };

    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("executed");
    let input_file = root.path().join("workflow.json");
    let executable = root.path().join("runner");
    let build = root.path().join("build");
    for streaming in [false, true] {
        let mut definition = definition(json!({"marker":marker}));
        if streaming {
            definition.execution = Some(serde_json::from_value(json!({"mode":"stream"})).unwrap());
        }
        fs::write(&input_file, serde_json::to_vec(&definition).unwrap()).unwrap();
        compile_project_with_options(
            &CompileRequest {
                definition: &input_file,
                output: &executable,
                locked: false,
                build_dir: Some(&build),
                support: &SupportPackages::Local {
                    crates_dir: common::crates_dir(),
                },
            },
            &RunnerOptions { telemetry: false },
        )
        .unwrap();
        assert!(!marker.exists());
        let registry = NodeRegistry::from_inventory().unwrap();
        let compiled = compile_definition(&definition, &registry).unwrap();
        let run = |flags: &[&str], drift: Option<&str>| {
            let mut command = Command::new(&executable);
            command.args(flags).stdin(Stdio::null());
            if let Some(drift) = drift {
                command.env(drift, "1");
            }
            command.output().unwrap()
        };
        let args = arguments().to_string();
        let output = run(
            &["--inputs", args.as_str()],
            Some("MF_FIXTURE_TYPED_INPUT_DRIFT"),
        );
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success());
        assert!(
            diagnostic.contains("interface drift") && diagnostic.contains("/typed.~1~0/count"),
            "{diagnostic}"
        );
        assert!(!marker.exists());
        let mut invalid = arguments();
        invalid["typed./~"]["rows./~"] = json!([{"count":1},{"count":"wrong"}]);
        let output = run(&["--inputs", &invalid.to_string()], None);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("/typed.~1~0/rows.~1~0/1/count"));
        assert!(!marker.exists());
        let mut non_finite = arguments();
        non_finite["typed./~"]["ratio"] = json!(-1.0);
        let output = run(&["--inputs", &non_finite.to_string()], None);
        assert!(!output.status.success());
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostic.contains("/sqrt") && diagnostic.contains("non-finite float"),
            "{diagnostic}"
        );
        assert!(output.stdout.is_empty());
        fs::remove_file(&marker).unwrap();
        for raw in [None, Some(json!(null)), Some(json!({"nested":[1,true]}))] {
            let mut args = arguments();
            if let Some(raw) = raw {
                args["typed./~"]["raw"] = raw;
            }
            let expected = if streaming {
                let instance = mf_compiler::instantiate_stream(&compiled, &registry)
                    .unwrap()
                    .start_with_options(mf_runtime::StreamOptions {
                        arguments: WorkflowArguments::try_from(args.clone()).unwrap(),
                        ..Default::default()
                    })
                    .unwrap();
                let outputs = instance.recv().unwrap().unwrap().outputs;
                assert!(instance.recv().unwrap().is_none());
                instance.join().unwrap();
                outputs
            } else {
                instantiate_compiled(&compiled, &registry)
                    .unwrap()
                    .execute_with_inputs(WorkflowArguments::try_from(args.clone()).unwrap())
                    .unwrap()
            };
            fs::remove_file(&marker).unwrap();
            let actual = run(&["--inputs", &args.to_string()], None);
            assert!(
                actual.status.success(),
                "{}",
                String::from_utf8_lossy(&actual.stderr)
            );
            assert_eq!(
                serde_json::from_slice::<Value>(&actual.stdout).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
            assert_eq!(fs::read(&marker).unwrap(), b"executed");
            fs::remove_file(&marker).unwrap();
        }
    }
}
