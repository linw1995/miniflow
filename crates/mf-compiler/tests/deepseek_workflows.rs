#![cfg(unix)]

mod common;
#[path = "common/tool_values.rs"]
mod tool_values;

extern crate mfn_code as _;
extern crate mfn_core as _;
extern crate mfn_deepseek as _;

use mf_compiler::{
    CompileRequest, NodeRegistry, RunnerOptions, SupportPackages, WorkflowDefinition,
    compile_definition, compile_project_with_options, instantiate_compiled,
};
use mf_runtime::{ExecutionContext, WorkflowArguments};
use mfn_deepseek::test_support::{Reply, Server};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command, time::Duration};

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
            },"inputs":{"query":{"$ref":"params.query"}}},
            {"id":"fetch","kind":"builtin.deepseek.fetch","config":{
                "base_url":{"$ref":"params.base_url"},
                "api_key":{"$ref":"params.api_key"},
                "timeout_ms":{"$ref":"params.timeout_ms"}
            },"inputs":{"url":{"$ref":"params.url"}}}
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

fn web_agent(server: &Server, fetch_timeout_ms: u64) -> Value {
    use std::collections::BTreeMap;
    use tool_values::{FlowTool, configured_agent};

    let mut tools = BTreeMap::new();
    for (name, argument, outputs) in [
        ("search", "query", vec!["results"]),
        ("fetch", "url", vec!["title", "content"]),
    ] {
        let mut body = json!({
            "version":"2026-10-28",
            "nodes":[
                {"id":"params","kind":"builtin.parameters","config":{"parameters":{
                    argument:{"type":"string","secret":false},
                    "base_url":{"type":"string","default":server.base_url},
                    "api_key":{"type":"string","default":"synthetic-tool-key"}
                }}},
                {"id":name,"kind":format!("builtin.deepseek.{name}"),"config":{
                    "base_url":{"$ref":"params.base_url"},
                    "api_key":{"$ref":"params.api_key"}
                },"inputs":{argument:{"$ref":format!("params.{argument}")}}}
            ],
            "outputs":outputs.into_iter().map(|port| json!({"name":port,"node":name,"port":port})).collect::<Vec<_>>()
        });
        if name == "fetch" {
            body["nodes"][1]["config"]["timeout_ms"] = fetch_timeout_ms.into();
        }
        tools.insert(
            name.into(),
            FlowTool {
                description: format!("Use DeepSeek {name}."),
                parameters: serde_json::from_value(
                    json!({argument:{"description":"The public request argument."}}),
                )
                .unwrap(),
                body: serde_json::from_value(body).unwrap(),
            },
        );
    }
    fn add_provider(value: &mut Value) {
        match value {
            Value::Object(fields) => {
                if fields.contains_key("version") && fields.contains_key("nodes") {
                    fields.get_mut("dependencies").unwrap()["deepseek"] = json!({
                        "package":"mfn-deepseek", "path":common::crates_dir().join("builtin-nodes/deepseek")
                    });
                }
                for child in fields.values_mut() {
                    add_provider(child);
                }
            }
            Value::Array(values) => values.iter_mut().for_each(add_provider),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(configured_agent(tools)).unwrap();
    add_provider(&mut value);
    value["outputs"]
        .as_array_mut()
        .unwrap()
        .retain(|output| output["name"] != "answer");
    value
}

fn model_tool_call(id: &str, name: &str, arguments: Value) -> mfn_llm::test_support::Reply {
    mfn_llm::test_support::Reply::json(json!({
        "id":"tool-response","object":"chat.completion","created":0,"model":"example-model",
        "choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{
            "id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}
        }]},"finish_reason":"tool_calls"}],
        "usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}
    }))
}

#[test]
fn checked_fetch_failure_reaches_agent_and_explicit_alternate_source_without_private_bindings() {
    let mut missing = Reply::json(json!({"error":{"message":"synthetic-tool-key remote-secret"}}));
    missing.status = 404;
    check_fetch_failure_recovery(missing, 60_000, "HTTP 404 Not Found");
}

#[test]
fn fetch_request_timeout_reaches_agent_and_explicit_alternate_source() {
    let mut stalled = Reply::fetch("Unavailable", "remote-secret");
    stalled.body_delay = Duration::from_millis(1_500);
    check_fetch_failure_recovery(
        stalled,
        1_000,
        "DeepSeek fetch request timed out after 1000 ms",
    );
}

fn check_fetch_failure_recovery(failure: Reply, fetch_timeout_ms: u64, expected_error: &str) {
    let results = json!([{
        "rerank_score":2.4,"title":"Release","url":"https://example.com/missing",
        "snippet":"Snippet","summary":"Summary","content":"Content","site_name":null,
        "site_icon":null,"date":null,"authority":3
    },{
        "rerank_score":2.0,"title":"Alternate","url":"https://example.com/alternate",
        "snippet":"Snippet","summary":"Summary","content":"Content","site_name":null,
        "site_icon":null,"date":null,"authority":3
    }]);
    let attempt = [
        Reply::search(results),
        failure,
        Reply::fetch("Alternate", "# Markdown"),
    ];
    let server = Server::start(attempt.iter().cloned().cycle().take(6).collect());
    let model = mfn_llm::test_support::Server::start(
        (0..2)
            .flat_map(|_| {
                [
                    model_tool_call("search", "search", json!({"query":"Release sources"})),
                    model_tool_call(
                        "missing",
                        "fetch",
                        json!({"url":"https://example.com/missing"}),
                    ),
                    model_tool_call(
                        "alternate",
                        "fetch",
                        json!({"url":"https://example.com/alternate"}),
                    ),
                    mfn_llm::test_support::Reply::text("Answer from the alternate source"),
                ]
            })
            .collect(),
    );
    let definition = web_agent(&server, fetch_timeout_ms);
    let root = tempfile::tempdir().unwrap();
    let executable = compile(&definition, root.path());
    assert!(server.requests().is_empty());
    assert!(model.requests().is_empty());
    let inputs = json!({
        "params.messages":[{"role":"user","content":"Find a readable release source."}],
        "params.model":"example-model", "params.base_url":model.base_url,
        "params.api_key":"synthetic-model-key"
    });
    let memory = run_memory(&definition, &inputs);
    fs::remove_file(root.path().join("flow.json")).unwrap();
    fs::remove_dir_all(root.path().join("build")).unwrap();
    let standalone = run(&executable, &inputs);
    for output in [memory, standalone] {
        assert_eq!(output["status"], "succeeded", "{output}");
        assert_eq!(output["text"], "Answer from the alternate source");
        assert_eq!(output["usages"].as_array().unwrap().len(), 4);
        let messages = &output["messages"];
        let transcript: Vec<mfn_llm::ChatMessage> =
            serde_json::from_value(messages.clone()).unwrap();
        mfn_llm::validate_transcript(&transcript, mfn_llm::TranscriptMode::Complete).unwrap();
        assert_eq!(transcript.len(), 7);
        assert_eq!(messages[3]["tool_call_id"], "missing");
        let failed: Value = serde_json::from_str(messages[3]["content"].as_str().unwrap()).unwrap();
        assert!(failed["error"].as_str().unwrap().contains(expected_error));
        assert_eq!(messages[5]["tool_call_id"], "alternate");
        assert_eq!(
            serde_json::from_str::<Value>(messages[5]["content"].as_str().unwrap()).unwrap(),
            json!({"title":"Alternate","content":"# Markdown"})
        );
        assert!(!output.to_string().contains("synthetic-tool-key"));
        assert!(!output.to_string().contains("remote-secret"));
    }
    let requests = model.requests();
    assert_eq!(requests.len(), 8);
    for round in requests.as_chunks::<4>().0 {
        assert!(
            round[2].body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| {
                    message["role"] == "tool"
                        && message["content"]
                            .as_str()
                            .is_some_and(|content| content.contains(expected_error))
                })
        );
        for request in round {
            let body = request.body.to_string();
            for private in [
                "synthetic-tool-key",
                "synthetic-model-key",
                "remote-secret",
                &server.base_url,
            ] {
                assert!(!body.contains(private), "{body}");
            }
            assert!(request.headers.contains("Bearer synthetic-model-key"));
            for tool in request.body["tools"].as_array().unwrap() {
                let expected = if tool["function"]["name"] == "fetch" {
                    json!(["url"])
                } else {
                    json!(["query"])
                };
                let properties = tool["function"]["parameters"]["properties"]
                    .as_object()
                    .unwrap();
                assert_eq!(json!(properties.keys().collect::<Vec<_>>()), expected);
            }
        }
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    for round in requests.as_chunks::<3>().0 {
        assert!(
            round[0]
                .headers
                .starts_with("POST /prefix/v1/tools/search ")
        );
        assert_eq!(round[1].body, json!({"url":"https://example.com/missing"}));
        assert_eq!(
            round[2].body,
            json!({"url":"https://example.com/alternate"})
        );
        for request in round {
            assert!(request.headers.contains("Bearer synthetic-tool-key"));
        }
    }
}

#[test]
fn empty_parameter_credentials_omit_authorization() {
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
    let inputs = json!({
        "params.base_url":server.base_url,
        "params.api_key":"",
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
    assert_eq!(run(&executable, &inputs), expected);
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    for request in &requests {
        assert!(
            !request
                .headers
                .to_ascii_lowercase()
                .contains("authorization:"),
            "{}",
            request.headers
        );
    }
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
