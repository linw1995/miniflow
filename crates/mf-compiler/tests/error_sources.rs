#[path = "fixtures/multi-nodes/src/line_producer.rs"]
mod line_producer;
#[path = "fixtures/multi-nodes/src/stream_fixture.rs"]
mod stream_fixture;
extern crate mfn_code as _;
extern crate mfn_core as _;

use mf_compiler::{
    BuildGuard, CompileRequest, ExecutionContext, FlowNode, Inputs, NodeBuildError,
    NodeExecutionError, NodeFactory, NodeMetadata, NodePorts, NodeRegistration, NodeRegistry,
    NodeResult, OutputDerivation, OutputDerivationError, PortSpec, PreparedNode, SupportPackages,
    TaskNode, TypeInferenceState, TypeMismatch, ValueType, WorkflowCompileError,
    WorkflowDefinition, compile_definition, compile_project, instantiate_compiled,
    instantiate_stream, plan_definition,
};
use serde_json::{Value, json};
use std::{error::Error, fs, fs::TryLockError, io};

fn find_source<'a, T: Error + 'static>(error: &'a (dyn Error + 'static)) -> Option<&'a T> {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(source) = error.downcast_ref::<T>() {
            return Some(source);
        }
        current = error.source();
    }
    None
}

struct FailWithIo;

impl TaskNode for FailWithIo {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        Err(Box::<dyn Error + Send + Sync>::from(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "denied by fixture",
        ))
        .into())
    }
}

fn fail_factory(_: Value) -> Result<PreparedNode, NodeBuildError> {
    Ok(PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(FailWithIo)),
        NodePorts {
            inputs: Vec::new(),
            outputs: vec![PortSpec::new("result", ValueType::Any, true)],
        },
    ))
}

inventory::submit! {
    NodeRegistration { kind: "test.error_source", factory: NodeFactory::Plain(fail_factory) }
}

fn loop_definition(body: Value) -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version": "2026-09-29", "dependencies": {},
        "nodes": [
            {"id": "seed", "kind": "builtin.constant", "config": {"value": 0}},
            {"id": "repeat", "kind": "workflow.loop", "loop": {
                "max_iterations": 1, "variables": [{"name": "x", "type": "int"}],
                "body": body
            }}
        ],
        "edges": [{"from_node": "seed", "from_output": "value", "to_node": "repeat", "to_input": "x"}],
        "outputs": [{"name": "x", "node": "repeat", "port": "x"}]
    })).unwrap()
}

#[test]
fn loop_failures_preserve_scope_context_and_typed_plugin_sources() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition =
        loop_definition(json!({"nodes": [{"id": "fail", "kind": "test.error_source"}]}));
    let plan = compile_definition(&definition, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let error = flow.execute().unwrap_err();
    assert!(error.to_string().contains("Loop `repeat` pass 0"));
    assert_eq!(
        find_source::<io::Error>(&error).unwrap().kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[test]
fn call_failures_preserve_typed_plugin_sources_and_call_context() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-25", "dependencies":{},
        "nodes":[{"id":"invoke","kind":"workflow.call","config":{
            "flow":"failing", "definition":{
                "version":"2026-10-25", "dependencies":{},
                "nodes":[{"id":"fail","kind":"test.error_source"}],
                "outputs":[{"name":"result","node":"fail","port":"result"}]
            }
        }}],
        "outputs":[{"name":"result","node":"invoke","port":"result"}]
    }))
    .unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let error = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap_err();
    assert!(error.to_string().contains("Flow call `invoke` (failing)"));
    assert_eq!(
        find_source::<io::Error>(&error).unwrap().kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[test]
fn iteration_failures_preserve_item_context_and_typed_workflow_sources() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for mode in ["sequential", "parallel"] {
        let definition: WorkflowDefinition = serde_json::from_value(json!({
            "version": "2026-09-26", "dependencies": {},
            "nodes": [
                {"id": "items", "kind": "builtin.constant", "config": {"value": [1]}},
                {"id": "each", "kind": "builtin.iteration", "config": {
                    "mode": mode, "body": {
                        "nodes": [{"id": "fail", "kind": "test.error_source"}],
                        "result": {"node": "fail", "port": "result"}
                    }
                }}
            ],
            "edges": [{"from_node": "items", "from_output": "value", "to_node": "each", "to_input": "items"}],
            "outputs": [{"name": "results", "node": "each", "port": "results"}]
        })).unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        let error = flow.execute().unwrap_err();
        assert!(error.to_string().contains("iteration item 0"));
        assert_eq!(
            find_source::<io::Error>(&error).unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}

#[test]
fn readline_conversion_preserves_the_stream_input_failure_phase() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("invalid.txt");
    fs::write(&path, b"\xff\n").unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-10-03", "execution": {}, "dependencies": {},
        "nodes": [{"id": "read", "kind": "builtin.readline"}],
        "outputs": [{"name": "text", "node": "read", "port": "line"}]
    }))
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let prepared = instantiate_stream(&plan, &registry).unwrap();
    let instance = prepared
        .start_with_options(mf_runtime::StreamOptions {
            arguments: mf_runtime::WorkflowArguments::try_from(json!({"read.path":path})).unwrap(),
            ..Default::default()
        })
        .unwrap();
    let error = instance.recv().unwrap_err();
    assert_eq!(error.phase(), "input");
    assert!(error.to_string().contains("line 1"));
    assert_eq!(instance.join().unwrap_err().phase(), "input");
}

#[test]
fn subgraph_compilation_preserves_nested_configuration_errors() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let broken = json!({"id": "broken", "kind": "builtin.constant", "config": {}});
    let loop_definition = loop_definition(json!({"nodes": [broken.clone()]}));
    let iteration_definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-09-26", "dependencies": {},
        "nodes": [
            {"id": "seed", "kind": "builtin.constant", "config": {"value": []}},
            {"id": "each", "kind": "builtin.iteration", "config": {"body": {
                "nodes": [broken], "result": {"node": "broken", "port": "value"}
            }}}
        ],
        "edges": [{"from_node": "seed", "from_output": "value", "to_node": "each", "to_input": "items"}]
    })).unwrap();
    for definition in [loop_definition, iteration_definition] {
        let error = compile_definition(&definition, &registry).unwrap_err();
        let source = find_source::<serde_json::Error>(&error).unwrap();
        assert!(source.is_data());
        assert!(source.to_string().contains("missing field `value`"));
        assert!(find_source::<Box<WorkflowCompileError>>(&error).is_some());
    }
}

#[test]
fn malformed_iteration_configuration_remains_a_typed_json_error() {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-09-26", "dependencies": {},
        "nodes": [{"id": "each", "kind": "builtin.iteration", "config": {"mode": "invalid"}}]
    }))
    .unwrap();
    let error = plan_definition(&definition).unwrap_err();
    assert!(error.to_string().contains("each"));
    assert!(find_source::<serde_json::Error>(&error).unwrap().is_data());
}

#[test]
fn malformed_loop_assignment_configuration_preserves_json_sources_and_scope() {
    let definition = loop_definition(json!({"nodes": [{
        "id": "assign", "kind": "workflow.loop_assign", "config": {}
    }]}));
    let error = plan_definition(&definition).unwrap_err();
    assert!(
        matches!(error, WorkflowCompileError::LoopAssignmentConfiguration { ref path, .. }
        if path == "[\"repeat\",\"assign\"]")
    );
    assert!(find_source::<serde_json::Error>(&error).unwrap().is_data());

    let definition = loop_definition(json!({"nodes": [{
        "id": "assign", "kind": "workflow.loop_assign", "config": {"variable": " "}
    }]}));
    let error = plan_definition(&definition).unwrap_err();
    assert!(matches!(error, WorkflowCompileError::InvalidLoop { .. }));
    assert!(
        error
            .to_string()
            .contains("assignment variable must not be blank")
    );
}

#[test]
fn loop_structure_errors_preserve_the_typed_inner_compilation_error() {
    let definition = loop_definition(json!({
        "nodes": [{"id": "assign", "kind": "workflow.loop_assign", "config": {"variable": "x"}}],
        "edges": [{"from_node": "missing", "from_output": "value", "to_node": "assign", "to_input": "value"}]
    }));
    let error = plan_definition(&definition).unwrap_err();
    assert!(
        matches!(error, WorkflowCompileError::LoopBody { ref path, .. }
        if path == "[\"repeat\"]")
    );
    assert!(
        matches!(find_source::<Box<WorkflowCompileError>>(&error).unwrap().as_ref(),
        WorkflowCompileError::UnknownEdgeSource { from_node, .. } if from_node.as_str() == "missing")
    );
}

fn inference_node(
    id: &str,
    output_type: ValueType,
    derivations: Vec<OutputDerivation>,
) -> FlowNode {
    FlowNode::new(
        id,
        PreparedNode::from_parts(
            mf_runtime::NodeExecution::Task(Box::new(FailWithIo)),
            NodeMetadata {
                output_derivations: derivations,
                ..NodeMetadata::new(NodePorts {
                    inputs: vec![PortSpec::new("input", ValueType::Any, false)],
                    outputs: vec![PortSpec::new("value", output_type, true)],
                })
            },
        ),
    )
}

#[test]
fn output_derivation_validation_preserves_typed_errors_and_nested_type_mismatches() {
    let mut node = inference_node(
        "bad",
        ValueType::List(Box::new(ValueType::Int64)),
        vec![OutputDerivation::literal("value", json!(["wrong"]))],
    );
    let error = TypeInferenceState::default()
        .resolve_node(&mut node, &[])
        .unwrap_err();
    assert!(
        matches!(error, WorkflowCompileError::OutputDerivation { ref definition_id, .. }
        if definition_id.as_str() == "bad")
    );
    assert!(matches!(
        find_source::<Box<OutputDerivationError>>(&error)
            .unwrap()
            .as_ref(),
        OutputDerivationError::LiteralTypeMismatch { .. }
    ));
    let mismatch = find_source::<TypeMismatch>(&error).unwrap();
    assert_eq!(mismatch.path, "/0");
    assert_eq!(mismatch.expected, ValueType::Int64);
}

#[test]
fn forwarded_known_output_values_preserve_typed_mismatches() {
    let mut inference = TypeInferenceState::default();
    let mut source = inference_node(
        "source",
        ValueType::Any,
        vec![OutputDerivation::literal("value", json!([1, "wrong"]))],
    );
    inference.resolve_node(&mut source, &[]).unwrap();
    let mut forward = inference_node(
        "forward",
        ValueType::List(Box::new(ValueType::Int64)),
        vec![OutputDerivation::forward_input("value", "input")],
    );
    let error = inference
        .resolve_node(
            &mut forward,
            &[mf_compiler::ExecutionDependency::Output {
                exclusive: false,
                source_node: "source",
                source_output: "value",
                input: Some("input"),
            }],
        )
        .unwrap_err();
    assert!(
        matches!(error, WorkflowCompileError::KnownOutputValueTypeConflict { ref definition_id, ref output, .. }
        if definition_id.as_str() == "forward" && output == "value")
    );
    let mismatch = find_source::<TypeMismatch>(&error).unwrap();
    assert_eq!(mismatch.path, "/1");
    assert_eq!(mismatch.expected, ValueType::Int64);
}

#[test]
fn lock_contention_preserves_the_typed_lock_error() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("busy.guard");
    let _guard = BuildGuard::acquire(&path).unwrap();
    let error = match BuildGuard::acquire(&path) {
        Ok(_) => panic!("expected lock contention"),
        Err(error) => error,
    };
    assert!(matches!(
        find_source::<TryLockError>(&error),
        Some(TryLockError::WouldBlock)
    ));
}

#[test]
fn fixture_io_failures_preserve_sources_during_construction_and_execution() {
    let root = tempfile::tempdir().unwrap();
    let blocked = root.path().join("blocked");
    fs::write(&blocked, "file").unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let registration = registry.get("fixture.stream_echo").unwrap();
    let error = registration
        .instantiate(json!({"trace": blocked.join("trace")}))
        .err()
        .unwrap();
    assert_eq!(
        find_source::<io::Error>(&error).unwrap().kind(),
        io::ErrorKind::NotADirectory
    );

    let trace = root.path().join("trace");
    let prepared = registration.instantiate(json!({"trace": trace})).unwrap();
    fs::remove_file(&trace).unwrap();
    fs::create_dir(&trace).unwrap();
    let error = prepared
        .execution
        .as_task_node()
        .unwrap()
        .execute(
            Inputs::from([("input".into(), json!(1).into())]),
            &mut ExecutionContext::default(),
        )
        .unwrap_err();
    assert_eq!(
        find_source::<io::Error>(&error).unwrap().kind(),
        io::ErrorKind::IsADirectory
    );
}

#[test]
fn filesystem_contexts_preserve_the_failing_path_and_io_source() {
    let root = tempfile::tempdir().unwrap();
    let definition = root.path().join("flow.json");
    fs::write(
        &definition,
        r#"{"version":"2026-09-26","dependencies":{},"nodes":[]}"#,
    )
    .unwrap();
    let blocked = root.path().join("blocked");
    fs::write(&blocked, "file").unwrap();
    let child = blocked.join("child");
    let inputs = mf_compiler::BuildInputs::new(&definition).unwrap();
    let error = inputs.check_output(&child).unwrap_err();
    assert!(matches!(&error, mf_compiler::InputError::Resolve { path, .. } if path == &child));
    assert_eq!(
        find_source::<io::Error>(&error).unwrap().kind(),
        io::ErrorKind::NotADirectory
    );

    let plan = plan_definition(
        &WorkflowDefinition::from_json(&fs::read_to_string(&definition).unwrap()).unwrap(),
    )
    .unwrap();
    let error = mf_compiler::write_dependency_project(&blocked, &plan, &SupportPackages::Registry)
        .unwrap_err();
    assert!(
        matches!(&error, mf_compiler::DependencyProjectError::Write { path, .. } if path == &blocked.join("src"))
    );
    assert_eq!(
        find_source::<io::Error>(&error).unwrap().kind(),
        io::ErrorKind::NotADirectory
    );

    let directory = root.path().join("managed");
    fs::create_dir(&directory).unwrap();
    let directory = fs::canonicalize(directory).unwrap();
    let marker = directory.join(".mf-owner.json");
    fs::create_dir(&marker).unwrap();
    let error = mf_compiler::BuildDirectory::open(&definition, Some(&directory))
        .err()
        .unwrap();
    assert!(matches!(&error, mf_compiler::CacheError::Io { path, .. } if path == &marker));
    assert_eq!(
        find_source::<io::Error>(&error).unwrap().kind(),
        io::ErrorKind::IsADirectory
    );
}

#[test]
fn native_producer_io_failures_preserve_the_original_source() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing.txt");
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-10-03", "execution": {}, "dependencies": {},
        "nodes": [
            {"id": "path", "kind": "builtin.constant", "config": {"value": missing}},
            {"id": "read", "kind": "fixture.read_lines", "config": {}}
        ],
        "edges": [{"from_node": "path", "from_output": "value", "to_node": "read", "to_input": "path"}],
        "outputs": [{"name": "line", "node": "read", "port": "line"}]
    })).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start()
        .unwrap();
    let error = instance.recv().unwrap_err();
    assert_eq!(error.phase(), "execution");
    assert_eq!(
        find_source::<io::Error>(&error).unwrap().kind(),
        io::ErrorKind::NotFound
    );
    assert!(instance.join().is_err());
}

#[test]
fn pipeline_context_preserves_definition_parse_errors() {
    let root = tempfile::tempdir().unwrap();
    let definition = root.path().join("invalid.json");
    let output = root.path().join("runner");
    fs::write(&definition, "{").unwrap();
    let support = SupportPackages::Local {
        crates_dir: root.path().to_owned(),
    };
    let error = compile_project(&CompileRequest {
        definition: &definition,
        output: &output,
        locked: false,
        build_dir: None,
        support: &support,
    })
    .unwrap_err();
    assert_eq!(error.stage, "definition parsing");
    assert!(find_source::<serde_json::Error>(&error).unwrap().is_eof());
}

#[test]
fn loop_validation_preserves_variable_and_descriptor_sources() {
    let mut definition = loop_definition(json!({"nodes": []}));
    definition.nodes[1]
        .loop_definition
        .as_mut()
        .unwrap()
        .variables[0]
        .value_type = json!("unknown");
    let error = plan_definition(&definition).unwrap_err();
    assert!(
        matches!(&error, WorkflowCompileError::LoopVariables { path, .. } if path == "[\"repeat\"]")
    );
    assert!(find_source::<mf_runtime::LoopVariableError>(&error).is_some());
    assert!(
        matches!(find_source::<mf_runtime::TypeDescriptorError>(&error).unwrap(),
        mf_runtime::TypeDescriptorError::UnsupportedName { name } if name == "unknown")
    );

    let error = mf_compiler::prepared_loop_source_from_json(r#"[{"name":"x","type":"unknown"}]"#)
        .err()
        .unwrap();
    assert!(find_source::<mf_runtime::LoopVariableError>(&error).is_some());
    assert!(find_source::<mf_runtime::TypeDescriptorError>(&error).is_some());
    let error = mf_compiler::prepared_loop_assign_from_json("assign", "x", r#""unknown""#)
        .err()
        .unwrap();
    assert!(find_source::<mf_runtime::TypeDescriptorError>(&error).is_some());
}

#[test]
fn iteration_validation_preserves_the_structural_source() {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-09-26", "dependencies": {},
        "nodes": [{"id": "each", "kind": "builtin.iteration", "config": {"body": {
            "nodes": [{"id": "%iteration", "kind": "builtin.constant", "config": {"value": 1}}],
            "result": {"node": "%iteration", "port": "value"}
        }}}]
    }))
    .unwrap();
    let error = plan_definition(&definition).unwrap_err();
    assert!(
        matches!(&error, WorkflowCompileError::IterationValidation { definition_id, .. } if definition_id.as_str() == "each")
    );
    assert!(
        matches!(find_source::<mf_compiler::IterationValidationError>(&error).unwrap(),
        mf_compiler::IterationValidationError::ReservedNode { node } if node.as_str() == "%iteration")
    );
}
