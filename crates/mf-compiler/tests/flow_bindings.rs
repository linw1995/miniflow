extern crate mfn_core as _;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled, load_workflow,
    resolve_flows,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn template() -> Result<WorkflowDefinition, mf_runtime::DefinitionParseError> {
    WorkflowDefinition::from_json(
        &json!({"version":"2026-10-31","dependencies":{},"subgraphs":{"tools":{"parameter":"single"}},"nodes":[
        {"id":"invoke","kind":"workflow.call","config":{"flow":"tools"}}
    ],"outputs":[{"name":"value","node":"invoke","port":"value"}]})
        .to_string(),
    )
}
mf_runtime::register_builtin_flow!("binding-template", template);

fn constant(value: i64) -> Value {
    json!({"version":"2026-10-31","dependencies":{},"nodes":[{"id":"value","kind":"builtin.constant","config":{"value":value}}],"outputs":[{"name":"value","node":"value","port":"value"}]})
}
fn caller(source: Value, binding: Value) -> Value {
    json!({"version":"2026-10-31","dependencies":{},"subgraphs":{"template":source},"nodes":[
        {"id":"agent","kind":"workflow.call","config":{"flow":"template"},"subgraphs":{"tools":binding}}
    ],"outputs":[{"name":"value","node":"agent","port":"value"}]})
}
fn execute(definition: &WorkflowDefinition) -> Value {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(definition, &registry).unwrap();
    serde_json::to_value(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap(),
    )
    .unwrap()
}
fn write(path: &Path, value: &Value) {
    fs::write(path, value.to_string()).unwrap();
}

#[test]
fn caller_binds_builtin_flow_imports_and_freezes_source_definitions() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("tools.json"), &constant(42));
    write(
        &root.path().join("flow.json"),
        &caller(
            json!({"builtin":"mf-compiler.binding-template"}),
            json!({"path":"tools.json"}),
        ),
    );
    let resolved = load_workflow(&root.path().join("flow.json")).unwrap();
    assert_eq!(resolved.sources.len(), 2);
    let encoded = serde_json::to_value(&resolved.definition).unwrap();
    assert!(encoded["nodes"][0]["config"].get("flows").is_none());
    assert!(
        encoded["nodes"][0]["config"]["definition"]
            .get("flows")
            .is_none()
    );
    fs::remove_file(root.path().join("tools.json")).unwrap();
    fs::remove_file(root.path().join("flow.json")).unwrap();
    assert_eq!(execute(&resolved.definition), json!({"value":42}));
}

#[test]
fn path_flow_binding_is_relative_to_caller_and_overrides_the_child_source() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("child")).unwrap();
    write(&root.path().join("tools.json"), &constant(42));
    write(&root.path().join("child/tools.json"), &constant(-1));
    let mut child = serde_json::to_value(template().unwrap()).unwrap();
    child["subgraphs"] = json!({"tools":{"parameter":"single","default":{"path":"tools.json"}}});
    write(&root.path().join("child/template.json"), &child);
    write(
        &root.path().join("flow.json"),
        &caller(
            json!({"path":"child/template.json"}),
            json!({"path":"tools.json"}),
        ),
    );
    let resolved = load_workflow(&root.path().join("flow.json")).unwrap();
    assert_eq!(execute(&resolved.definition), json!({"value":42}));
    assert!(
        !resolved
            .sources
            .contains(&fs::canonicalize(root.path().join("child/tools.json")).unwrap())
    );
}

#[test]
fn absolute_binding_can_be_resolved_without_a_source_directory() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("tools.json"), &constant(7));
    let definition: WorkflowDefinition = serde_json::from_value(caller(
        json!({"builtin":"mf-compiler.binding-template"}),
        json!({"path":root.path().join("tools.json")}),
    ))
    .unwrap();
    let resolved = resolve_flows(&definition, None).unwrap();
    assert_eq!(execute(&resolved.definition), json!({"value":7}));
    let relative: WorkflowDefinition = serde_json::from_value(caller(
        json!({"builtin":"mf-compiler.binding-template"}),
        json!({"path":"tools.json"}),
    ))
    .unwrap();
    assert!(
        resolve_flows(&relative, None)
            .err()
            .unwrap()
            .to_string()
            .contains("relative subgraph sources require a source directory")
    );
}

#[cfg(feature = "codegen")]
#[test]
fn generated_flow_parameters_accept_nodes_and_flows_without_source_files() {
    use mf_compiler::{CompileRequest, SupportPackages, compile_project};
    use std::process::Command;
    let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for node in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let operation = json!({"id":"operation","kind":"builtin.code","config":{"language":"cel","inputs":{"x":"int"},"code":{"value":"x + 1"}}});
        let source = if node {
            json!({"node":operation})
        } else {
            json!({"source":{"definition":{"version":"2026-10-31","dependencies":{},"nodes":[operation],"outputs":[{"name":"value","node":"operation","port":"value"}]}},"inputs":{"x":"operation.x"}})
        };
        let mut child = serde_json::to_value(template().unwrap()).unwrap();
        child["nodes"][0]["inputs"] = json!({"x":{"from":{"node":"seed","port":"value"}}});
        child["nodes"].as_array_mut().unwrap().insert(
            0,
            json!({"id":"seed","kind":"builtin.constant","config":{"value":41}}),
        );
        let mut workflow = caller(json!({"path":"template.json"}), source);
        workflow["dependencies"] = json!({"core":{"package":"mfn-core","path":crates_dir.join("builtin-nodes/core")},"code":{"package":"mfn-code","path":crates_dir.join("builtin-nodes/code")}});
        write(&root.path().join("template.json"), &child);
        let source = root.path().join("workflow.json");
        write(&source, &workflow);
        let binary = root.path().join("runner");
        let build = root.path().join("build");
        compile_project(&CompileRequest {
            definition: &source,
            output: &binary,
            locked: false,
            build_dir: Some(&build),
            support: &SupportPackages::Local {
                crates_dir: crates_dir.into(),
            },
        })
        .unwrap();
        fs::remove_file(source).unwrap();
        fs::remove_file(root.path().join("template.json")).unwrap();
        fs::remove_dir_all(build).unwrap();
        let output = Command::new(&binary).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({"value":42})
        );
    }
}
