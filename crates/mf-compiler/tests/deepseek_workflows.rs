#![cfg(unix)]

mod common;

extern crate mfn_core as _;
extern crate mfn_deepseek as _;

use mf_compiler::{
    CompileRequest, NodeRegistry, RunnerOptions, SupportPackages, WorkflowDefinition,
    compile_definition, compile_project_with_options, instantiate_compiled,
};
use mf_runtime::{ExecutionContext, WorkflowArguments};
use mfn_deepseek::test_support::{Reply, Server};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn definition() -> Value {
    json!({
        "version":"2026-10-23",
        "dependencies":{
            "core":{"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")},
            "deepseek":{"package":"mfn-deepseek","path":common::crates_dir().join("builtin-nodes/deepseek")}
        },
        "nodes":[
            {"id":"params","kind":"builtin.parameters","config":{"parameters":{
                "base_url":{"type":"string","secret":false},
                "api_key":{"type":"string"},
                "query":{"type":"string","secret":false},
                "url":{"type":"string","secret":false},
                "timeout_ms":{"type":"uint","default":60000,"secret":false}
            }}},
            {"id":"search","kind":"builtin.deepseek.search","config":{
                "base_url":{"$ref":"params.base_url"},
                "api_key":{"$ref":"params.api_key"},
                "timeout_ms":{"$ref":"params.timeout_ms"}
            },"inputs":{"query":{"from":{"node":"params","port":"query"}}}},
            {"id":"fetch","kind":"builtin.deepseek.fetch","config":{
                "base_url":{"$ref":"params.base_url"},
                "api_key":{"$ref":"params.api_key"},
                "timeout_ms":{"$ref":"params.timeout_ms"}
            },"inputs":{"url":{"from":{"node":"params","port":"url"}}}}
        ],
        "edges":[{"from_node":"search","to_node":"fetch"}],
        "outputs":[
            {"name":"results","node":"search","port":"results"},
            {"name":"title","node":"fetch","port":"title"},
            {"name":"content","node":"fetch","port":"content"}
        ]
    })
}

fn compile(value: &Value, root: &Path) -> std::path::PathBuf {
    let definition = root.join("flow.json");
    let output = root.join("flow");
    fs::write(&definition, value.to_string()).unwrap();
    compile_project_with_options(
        &CompileRequest {
            definition: &definition,
            output: &output,
            locked: false,
            build_dir: Some(&root.join("build")),
            support: &SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        },
        &RunnerOptions { telemetry: false },
    )
    .unwrap();
    output
}

fn arguments(value: &Value) -> WorkflowArguments {
    WorkflowArguments::from_json(value.to_string().as_bytes()).unwrap()
}

fn run_memory(value: &Value, inputs: &Value) -> Value {
    let definition: WorkflowDefinition = serde_json::from_value(value.clone()).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let mut context = ExecutionContext::default();
    context.set_workflow_arguments(arguments(inputs));
    let output = flow.execute_in_context(&mut context).unwrap();
    serde_json::to_value(output).unwrap()
}

fn run(executable: &Path, inputs: &Value) -> Value {
    let output = Command::new(executable)
        .current_dir(executable.parent().unwrap())
        .args(["--inputs", &inputs.to_string()])
        .env_clear()
        .env("PATH", "")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn search_and_fetch_match_memory_and_standalone_runners() {
    let result = json!({
        "rerank_score":2.4,"title":"DeepSeek release","url":"https://example.com/release",
        "snippet":"Snippet","summary":"Summary","content":"Content","site_name":null,
        "site_icon":null,"date":null,"authority":3
    });
    let server = Server::start(vec![
        Reply::search(json!([result.clone()])),
        Reply::fetch("Example", "# Markdown"),
        Reply::search(json!([result.clone()])),
        Reply::fetch("Example", "# Markdown"),
    ]);
    let definition = definition();
    let root = tempfile::tempdir().unwrap();
    let executable = compile(&definition, root.path());
    assert!(server.requests().is_empty());
    let inputs = json!({
        "params.base_url":server.base_url,
        "params.api_key":"synthetic-template-key",
        "params.query":"DeepSeek",
        "params.url":"https://example.com/article",
        "params.timeout_ms":5000
    });
    let expected = json!({
        "results":[result],
        "title":"Example",
        "content":"# Markdown"
    });
    assert_eq!(run_memory(&definition, &inputs), expected);
    fs::remove_file(root.path().join("flow.json")).unwrap();
    fs::remove_dir_all(root.path().join("build")).unwrap();
    assert_eq!(run(&executable, &inputs), expected);
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    for pair in requests.as_chunks::<2>().0 {
        assert!(
            pair[0]
                .headers
                .starts_with("POST /prefix/v1/tools/search HTTP/1.1")
        );
        assert!(
            pair[1]
                .headers
                .starts_with("POST /prefix/v1/tools/fetch HTTP/1.1")
        );
        for request in pair {
            assert!(request.headers.contains("Bearer synthetic-template-key"));
        }
    }
}
