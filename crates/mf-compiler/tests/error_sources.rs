extern crate mfn_code as _;
extern crate mfn_core as _;

use mf_compiler::{
    BuildGuard, CompileRequest, ExecutionContext, Inputs, NodeBuildError, NodeExecutionError,
    NodeFactory, NodePorts, NodeRegistration, NodeRegistry, NodeResult, PreparedNode,
    SupportPackages, TaskNode, WorkflowCompileError, WorkflowDefinition, compile_definition,
    compile_project, instantiate_compiled, plan_definition,
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
        use snafu::ResultExt;
        Err(Box::new(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "denied by fixture",
        )) as Box<dyn Error + Send + Sync>)
        .context(mf_runtime::NodePluginFailedSnafu)
    }
}

fn fail_factory(_: Value) -> Result<PreparedNode, NodeBuildError> {
    Ok(PreparedNode::new(FailWithIo, NodePorts::default()))
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
