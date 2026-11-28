#![cfg(feature = "development-support")]

#[path = "support/examples.rs"]
mod example_support;
use example_support::{CODING_AGENT, streamed_tool_calls};
use mfn_deepseek::test_support as deepseek_support;
use mfn_llm::test_support as llm_support;
use serde_json::{Value, json};
use std::{fs, path::Path};

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
}

fn chat_usage(path: Value) -> Value {
    json!({"started_at":0,"call_id":"generated", "node":{"id":"chat", "kind":"builtin.llm", "path":path}, "model":"example-model", "usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}})
}

fn title_usage(node: &str, purpose: &str) -> Value {
    let mut usage = chat_usage(json!([]));
    usage["node"]["id"] = node.into();
    usage["attribution"] = json!({"purpose":purpose});
    usage
}

fn nested_title_usage() -> Value {
    let mut usage = title_usage("title_model", "title");
    usage["node"]["path"] = json!([{"node":"title","index":0}]);
    usage
}

const CALCULATOR_DESCRIPTION: &str = "Set operation to add, subtract, multiply, or divide on the numeric operands left and right. Division by zero returns an error message.";
const CALCULATOR_PROMPT: &str = "Calculate 2 + 3 and 3 * 4.";
const CALCULATOR_ANSWER: &str = "The results are 5 and 12.";
const CALCULATOR_SUMMARY: &str = "Calculator findings: 2 + 3 = 5; 3 * 4 = 12.";

fn agent_usage(iteration: usize) -> Value {
    let mut usage = chat_usage(json!([
        {"node":"agent","index":0},
        {"node":"repeat","index":iteration},
        {"node":"turn","index":0}
    ]));
    usage["node"]["id"] = "model".into();
    usage["attribution"] = json!({"purpose":"conversation"});
    usage
}

fn context_agent_usage(iteration: usize) -> Value {
    let mut usage = agent_usage(iteration);
    usage["node"]["path"]
        .as_array_mut()
        .unwrap()
        .insert(0, json!({"node":"conversation","index":0}));
    usage
}

fn calculator_calls() -> Value {
    json!([
        {"id":"call_add","type":"function","function":{"name":"calculator","arguments":"{\"operation\":\"add\",\"left\":2.0,\"right\":3.0}"}},
        {"id":"call_multiply","type":"function","function":{"name":"calculator","arguments":"{\"operation\":\"multiply\",\"left\":3.0,\"right\":4.0}"}}
    ])
}

fn streamed_calculator_calls() -> llm_support::Reply {
    streamed_tool_calls(calculator_calls())
}

fn append_result(messages: Value, usages: Value) -> Value {
    json!({"append":{"status":"succeeded","messages":messages,"usages":usages,"error":""}})
}

fn assistant(text: &str) -> Value {
    json!({"role":"assistant","content":text})
}

fn calculator_messages() -> Value {
    json!([
        {"role":"assistant","content":"","tool_calls":[
            {"id":"call_add","name":"calculator","arguments":{"operation":"add","left":2.0,"right":3.0}},
            {"id":"call_multiply","name":"calculator","arguments":{"operation":"multiply","left":3.0,"right":4.0}}
        ]},
        {"role":"tool","tool_call_id":"call_add","content":"5.0"},
        {"role":"tool","tool_call_id":"call_multiply","content":"12.0"},
        assistant(CALCULATOR_ANSWER)
    ])
}

fn title_result(usages: Value) -> Value {
    let mut result = append_result(json!([assistant("reply")]), usages);
    result["append"]["title"] = "reply".into();
    result
}

fn search_result() -> Value {
    json!({
        "rerank_score":2.4,"title":"DeepSeek release","url":"https://example.com/release",
        "snippet":"Snippet","summary":"Summary","content":"Content","site_name":null,
        "site_icon":null,"date":null,"authority":3
    })
}

fn deepseek_calls(fetch: bool) -> Value {
    if fetch {
        json!([{"id":"call_fetch","type":"function","function":{"name":"fetch","arguments":"{\"url\":\"https://example.com/release\"}"}}])
    } else {
        json!([
            {"id":"call_search_a","type":"function","function":{"name":"search","arguments":"{\"query\":\"DeepSeek Search API\",\"count\":2}"}},
            {"id":"call_search_b","type":"function","function":{"name":"search","arguments":"{\"query\":\"DeepSeek Fetch API\",\"count\":2}"}}
        ])
    }
}

fn deepseek_messages() -> Value {
    let assistant_calls = |fetch| {
        let calls: Vec<_> = deepseek_calls(fetch).as_array().unwrap().iter().map(|call| {
            json!({"id":call["id"], "name":call["function"]["name"], "arguments":serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap()).unwrap()})
        }).collect();
        json!({"role":"assistant", "content":"", "tool_calls":calls})
    };
    let results = json!({"results":[search_result()]}).to_string();
    json!([
        assistant_calls(false),
        {"role":"tool","tool_call_id":"call_search_a","content":results},
        {"role":"tool","tool_call_id":"call_search_b","content":results},
        assistant_calls(true),
        {"role":"tool","tool_call_id":"call_fetch","content":json!({"title":"Example","content":"# Markdown"}).to_string()},
        assistant("DeepSeek findings: https://example.com/release")
    ])
}

fn examples() -> [(&'static str, Option<&'static str>, Value); 27] {
    [
        (
            "deepseek-fetch.json",
            None,
            json!({"title":"Example","content":"# Markdown"}),
        ),
        (
            "deepseek-search.json",
            None,
            json!({"results":[search_result()]}),
        ),
        ("cel-list.json", None, json!({"doubled": [2, 4]})),
        ("cel-scalar.json", None, json!({"doubled": 42})),
        ("clustered-workflow.json", None, json!({"doubled": 42})),
        (
            "coding-read.json",
            None,
            json!({"content":"second\n", "next_line":3}),
        ),
        (
            "chat-calculator-flow.json",
            None,
            append_result(
                calculator_messages(),
                json!([agent_usage(0), agent_usage(1)]),
            ),
        ),
        (
            CODING_AGENT,
            None,
            append_result(
                json!([{"role":"assistant", "content":"reply", "metadata":{"coding_workspace":"$WORKSPACE"}}]),
                json!([agent_usage(0)]),
            ),
        ),
        (
            "chat-context-flow.json",
            None,
            json!({"replace_context":{
                "status":"succeeded",
                "context_messages":[{"role":"system","content":CALCULATOR_SUMMARY}],
                "display_messages":calculator_messages(),
                "usages":[context_agent_usage(0), context_agent_usage(1), title_usage("summary", "compaction")],
                "error":""
            }}),
        ),
        (
            "chat-deepseek-flow.json",
            None,
            append_result(
                deepseek_messages(),
                json!([agent_usage(0), agent_usage(1), agent_usage(2)]),
            ),
        ),
        (
            "chat-flow.json",
            None,
            append_result(json!([assistant("reply")]), json!([chat_usage(json!([]))])),
        ),
        (
            "chat-loop-flow.json",
            None,
            append_result(
                json!([
                    {"role":"assistant","content":"reply","metadata":{"strategy":"iterative"}},
                    {"role":"assistant","content":"reply","metadata":{"strategy":"iterative"}}
                ]),
                json!([
                    chat_usage(json!([{"node":"repeat","index":0}])),
                    chat_usage(json!([{"node":"repeat","index":1}]))
                ]),
            ),
        ),
        (
            "chat-title-flow.json",
            None,
            title_result(json!([
                title_usage("model", "conversation"),
                title_usage("title_model", "title")
            ])),
        ),
        (
            "chat-nested-title-flow.json",
            None,
            title_result(json!([
                title_usage("model", "conversation"),
                nested_title_usage()
            ])),
        ),
        ("else-if.json", None, json!({"medium": {"amount": 500}})),
        (
            "exclusive-inputs.json",
            None,
            json!({"result":{"mode":"local"}}),
        ),
        ("hello-workflow.json", None, json!({"answer": 42})),
        ("if-else.json", None, json!({"accepted": {"amount": 150}})),
        ("iteration.json", None, json!({"results": [2, 5, 8]})),
        ("loop.json", None, json!({"count": 3})),
        ("loop-outputs.json", None, json!({"count": 3})),
        ("variables.json", None, json!({"count": 7, "observed": 7})),
        (
            "llm-terminal.json",
            None,
            json!({"text":"reply", "message":{"role":"assistant","content":"reply"}}),
        ),
        (
            "llm-parameters.json",
            None,
            json!({"text":"reply", "message":{"role":"assistant","content":"reply"}}),
        ),
        (
            "llm-loop.json",
            None,
            json!({"text": "reply", "history": [{"role": "user", "content": "Hello"}, {"role": "assistant", "content": "reply"}, {"role": "user", "content": "Hello"}, {"role": "assistant", "content": "reply"}]}),
        ),
        (
            "llm-tools.json",
            None,
            json!({"outcome":"completed", "text":"reply", "message":{"role":"assistant","content":"reply"}, "tool_calls":[], "usages":[{
                "started_at":0, "call_id":"generated", "node":{"id":"model","kind":"builtin.llm","path":[]}, "model":"example-model", "usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}
            }]}),
        ),
        ("stream-batch.json", Some("1\n"), json!({"batch": [1]})),
    ]
}

#[test]
fn every_example_has_an_expected_result() {
    let mut actual: Vec<_> = fs::read_dir(workspace().join("examples"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
        .collect();
    let mut expected: Vec<_> = examples().into_iter().map(|(name, _, _)| name).collect();
    actual.sort();
    expected.sort();
    assert_eq!(
        actual, expected,
        "update the example cases when adding, removing, or renaming examples/*.json"
    );
}

#[cfg(unix)]
fn assert_agent_layouts(executable: &Path, routes: &[&str]) {
    use mf_telemetry::event::ScopePathEntry;
    use mf_tui::graph::{BodyDisplay, ConnectionView, GraphLayout};
    use std::collections::{BTreeMap, BTreeSet};

    let description = mf_tui::manifest::read_manifest(executable)
        .unwrap()
        .description()
        .unwrap();
    let scopes: &[(&[&str], usize, &[&str])] = &[
        (&["agent"], 3, &["repeat", "result"]),
        (&["agent", "catalog", "project"], 2, &["definition"]),
        (&["agent", "repeat"], 6, &["request", "turn", "dispatch"]),
        (&["agent", "repeat", "request"], 1, &[]),
        (&["agent", "repeat", "dispatch"], 2, &["dispatch"]),
        (
            &["agent", "repeat", "dispatch", "dispatch"],
            3,
            &["execute"],
        ),
    ];
    for &(path, boxes, visible) in scopes {
        let path = path.iter().map(ToString::to_string).collect::<Vec<_>>();
        let body = description.body(&path).unwrap();
        let layout =
            GraphLayout::from_body_with_view(&description, &path, ConnectionView::Execution)
                .unwrap();
        assert_eq!(
            layout.nodes().len(),
            boxes + usize::from(body.source.is_some()),
            "{path:?}"
        );
        for id in visible {
            let display = layout.display_index_for_node_id(id).unwrap();
            assert_eq!(layout.nodes()[display].id, *id, "{path:?}");
        }
        let expanded = GraphLayout::from_body_with_view_and_collapsed(
            &description,
            &path,
            ConnectionView::Execution,
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(expanded.nodes().len(), body.nodes.len(), "{path:?}");
    }

    let agent = vec!["agent".to_owned()];
    let default_states: BTreeMap<_, _> = description
        .loop_bodies
        .iter()
        .map(|body| (body.path.clone(), BodyDisplay::Summary))
        .collect();
    let summary = GraphLayout::with_body_states(
        &description,
        ConnectionView::Execution,
        &default_states,
        &BTreeMap::new(),
    )
    .unwrap();
    let owner = summary.display_index_for_node_id("agent").unwrap();
    assert_eq!(summary.body_display(owner), Some(BodyDisplay::Summary));
    assert!(
        summary
            .display_index_for_scoped_node(&agent, "repeat")
            .is_none()
    );
    let mut count = description.nodes.len();
    for route in routes {
        let path = vec![
            "agent".into(),
            "repeat".into(),
            "dispatch".into(),
            "dispatch".into(),
            ScopePathEntry::binding_key("execute", route),
        ];
        let body = description.body(&path).unwrap();
        for node in &body.nodes {
            assert!(
                summary
                    .display_index_for_scoped_node(&path, &node.id)
                    .is_some(),
                "missing {route}/{}",
                node.id
            );
        }
        count += body.nodes.len();
    }
    assert_eq!(summary.nodes().len(), count);
    mf_tui::run::ObservationView::new(&description).unwrap();
}

#[cfg(unix)]
#[test]
fn documented_examples_compile_and_produce_expected_outputs() {
    use std::process::Command;

    let temporary = tempfile::Builder::new()
        .prefix("mf examples ")
        .tempdir()
        .unwrap();
    let runtime = temporary.path().join("runtime");
    fs::create_dir(&runtime).unwrap();

    for (name, input, expected) in examples() {
        let sources = tempfile::tempdir_in(temporary.path()).unwrap();
        let definition = example_support::stage_example(sources.path(), name);
        let server = (name.starts_with("llm-") || name.starts_with("chat-")).then(|| {
            llm_support::Server::start(if name == "chat-calculator-flow.json" {
                vec![
                    streamed_calculator_calls(),
                    llm_support::Reply::stream_text(CALCULATOR_ANSWER),
                ]
            } else if name == "chat-deepseek-flow.json" {
                vec![
                    streamed_tool_calls(deepseek_calls(false)),
                    streamed_tool_calls(deepseek_calls(true)),
                    llm_support::Reply::stream_text(
                        "DeepSeek findings: https://example.com/release",
                    ),
                ]
            } else if name == "chat-context-flow.json" {
                vec![
                    streamed_calculator_calls(),
                    llm_support::Reply::stream_text(CALCULATOR_ANSWER),
                    llm_support::Reply::text(CALCULATOR_SUMMARY),
                ]
            } else if matches!(name, "chat-title-flow.json" | "chat-nested-title-flow.json") {
                vec![
                    llm_support::Reply::stream_text("reply"),
                    llm_support::Reply::text("reply"),
                ]
            } else if name == CODING_AGENT {
                vec![llm_support::Reply::stream_text("reply")]
            } else if name == "chat-loop-flow.json" {
                vec![llm_support::Reply::stream_text("reply"); 2]
            } else {
                vec![llm_support::Reply::text("reply"); 2]
            })
        });
        let deepseek_server = (name.starts_with("deepseek-") || name == "chat-deepseek-flow.json")
            .then(|| {
                let replies = if name == "chat-deepseek-flow.json" {
                    vec![
                        deepseek_support::Reply::search(json!([search_result()])),
                        deepseek_support::Reply::search(json!([search_result()])),
                        deepseek_support::Reply::fetch("Example", "# Markdown"),
                    ]
                } else if name == "deepseek-search.json" {
                    vec![deepseek_support::Reply::search(json!([search_result()]))]
                } else {
                    vec![deepseek_support::Reply::fetch("Example", "# Markdown")]
                };
                deepseek_support::Server::start(replies)
            });
        if let Some(server) = &server
            && !name.starts_with("chat-")
        {
            let mut value: Value = serde_json::from_slice(&fs::read(&definition).unwrap()).unwrap();
            let config = if name == "llm-loop.json" {
                &mut value["nodes"][0]["config"]["body"]["nodes"][0]["config"]
            } else if name == "llm-parameters.json" {
                &mut value["nodes"][1]["config"]
            } else {
                &mut value["nodes"][0]["config"]
            };
            config["base_url"] = server.base_url.clone().into();
            config["model"] = "example-model".into();
            fs::write(&definition, value.to_string()).unwrap();
        }
        let stem = Path::new(name).file_stem().unwrap();
        let executable = runtime.join(stem);
        let build = temporary.path().join("build").join(stem);
        let compiled = Command::new(env!("CARGO_BIN_EXE_mf"))
            .current_dir(&runtime)
            .env("MF_DEV_SUPPORT_ROOT", workspace().join("crates"))
            .env("CARGO_NET_OFFLINE", "true")
            .env("CARGO_TERM_COLOR", "never")
            .env_remove("OPENAI_API_KEY")
            .arg("compile")
            .arg(&definition)
            .arg("--output")
            .arg(&executable)
            .arg("--build-dir")
            .arg(&build)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{name}: compilation failed with {}:\n{}\n{}",
            compiled.status,
            String::from_utf8_lossy(&compiled.stdout),
            String::from_utf8_lossy(&compiled.stderr),
        );

        let stderr = String::from_utf8_lossy(&compiled.stderr);
        assert!(
            !stderr
                .lines()
                .any(|line| line.starts_with("warning:") || line.starts_with("warning[")),
            "{name}: compilation emitted warnings:\n{stderr}",
        );

        // Agent turns and tool dispatch must work without their original Flow sources.
        drop(sources);
        if matches!(
            name,
            "chat-calculator-flow.json"
                | "chat-context-flow.json"
                | "chat-deepseek-flow.json"
                | CODING_AGENT
        ) {
            fs::remove_dir_all(&build).unwrap();
            if name == "chat-context-flow.json" {
                use mf_telemetry::event::ScopePathEntry;
                use mf_tui::graph::{BodyDisplay, ConnectionView, GraphLayout};
                use std::collections::BTreeMap;

                let description = mf_tui::manifest::read_manifest(&executable)
                    .unwrap()
                    .description()
                    .unwrap();
                let default_states: BTreeMap<_, _> = description
                    .loop_bodies
                    .iter()
                    .map(|body| (body.path.clone(), BodyDisplay::Summary))
                    .collect();
                let summary = GraphLayout::with_body_states(
                    &description,
                    ConnectionView::Execution,
                    &default_states,
                    &BTreeMap::new(),
                )
                .unwrap();
                let conversation = summary.display_index_for_node_id("conversation").unwrap();
                assert_eq!(
                    summary.body_display(conversation),
                    Some(BodyDisplay::Summary)
                );
                assert!(
                    summary
                        .display_index_for_scoped_node(&["conversation".into()], "agent")
                        .is_none()
                );
                let calculator = vec![
                    "conversation".into(),
                    "agent".into(),
                    "repeat".into(),
                    "dispatch".into(),
                    "dispatch".into(),
                    ScopePathEntry::binding_key("execute", "calculator"),
                ];
                let body = description.body(&calculator).unwrap();
                for node in &body.nodes {
                    assert!(
                        summary
                            .display_index_for_scoped_node(&calculator, &node.id)
                            .is_some(),
                        "missing calculator/{}",
                        node.id
                    );
                }
                assert_eq!(
                    summary.nodes().len(),
                    description.nodes.len() + body.nodes.len()
                );
                mf_tui::run::ObservationView::new(&description).unwrap();
            } else if name == "chat-deepseek-flow.json" {
                assert_agent_layouts(&executable, &["search", "fetch"]);
            } else if name == CODING_AGENT {
                assert_agent_layouts(&executable, &["bash", "edit", "read", "write"]);
            } else {
                assert_agent_layouts(&executable, &["calculator"]);
            }
        }
        let mut command = Command::new(&executable);
        command.current_dir(&runtime).env("PATH", "");
        if name == "coding-read.json" {
            let root = temporary.path().join("coding-workspace");
            fs::create_dir(&root).unwrap();
            fs::write(root.join("input.txt"), "first\nsecond\nthird\n").unwrap();
            let inputs = json!({
                "params.workspace_root": root,
                "params.path": "input.txt",
                "params.start_line": 2,
                "params.limit": 1,
            });
            command.args(["--inputs", &inputs.to_string()]);
        }
        if let Some(server) = &server {
            let inputs = if name == "llm-loop.json" {
                json!({"repeat.prompt":"Hello", "repeat.history":[], "repeat.text":""})
            } else if matches!(name, "chat-calculator-flow.json" | "chat-context-flow.json") {
                let mut inputs = json!({
                    "params.messages":[
                        {"role":"system","content":"Use the calculator."},
                        {"role":"user","content":"Earlier question"},
                        {"role":"assistant","content":"Earlier answer"},
                        {"role":"user","content":CALCULATOR_PROMPT}
                    ],
                    "params.model":"example-model",
                    "params.base_url":server.base_url,
                    "params.api_key":"synthetic-calculator-key"
                });
                if name == "chat-context-flow.json" {
                    inputs["params.compact"] = true.into();
                }
                inputs
            } else if name == "chat-deepseek-flow.json" {
                json!({
                    "params.messages":[{"role":"user","content":"Search DeepSeek Search and Fetch APIs, then read a result."}],
                    "params.llm_model":"example-model",
                    "params.llm_base_url":server.base_url,
                    "params.llm_api_key":"synthetic-llm-key",
                    "params.deepseek_base_url":deepseek_server.as_ref().unwrap().base_url,
                    "params.deepseek_api_key":"synthetic-deepseek-key"
                })
            } else if name.starts_with("chat-") {
                let mut inputs = json!({"params.messages":[{"role":"user","content":"Hello"}], "params.model":"example-model", "params.base_url":server.base_url});
                if name == CODING_AGENT {
                    let root = temporary.path().join("chat-workspace");
                    fs::create_dir(&root).unwrap();
                    inputs["params.workspace_root"] = json!(root);
                }
                inputs
            } else if name == "llm-tools.json" {
                json!({"model.messages":[{"role":"user","content":"Hello"}]})
            } else if name == "llm-parameters.json" {
                json!({"params.messages":[{"role":"user","content":"Hello"}]})
            } else {
                json!({"chat.messages":[{"role":"user","content":"Hello"}]})
            };
            command
                .args(["--inputs", &inputs.to_string()])
                .env("OPENAI_API_KEY", "synthetic-key")
                .env("NO_PROXY", "127.0.0.1,localhost")
                .env("no_proxy", "127.0.0.1,localhost");
        }
        if name == "chat-deepseek-flow.json" {
            command
                .env("DEEPSEEK_BASE_URL", "http://127.0.0.1:1/poisoned")
                .env("DEEPSEEK_API_KEY", "wrong-deepseek-environment-key");
        }
        if let Some(server) = &deepseek_server
            && name.starts_with("deepseek-")
        {
            let inputs = if name == "deepseek-search.json" {
                json!({"params.base_url":server.base_url,"params.query":"DeepSeek"})
            } else {
                json!({"params.base_url":server.base_url,"params.url":"https://example.com/article"})
            };
            command
                .args(["--inputs", &inputs.to_string()])
                .env("DEEPSEEK_API_KEY", "synthetic-key")
                .env("NO_PROXY", "127.0.0.1,localhost")
                .env("no_proxy", "127.0.0.1,localhost");
        }
        if let Some(data) = input {
            let path = runtime.join("lines.txt");
            fs::write(&path, data).unwrap();
            command.arg("--feed.path").arg(path);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{name}: execution failed with {}:\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let mut actual: Value = if matches!(
            name,
            "chat-title-flow.json"
                | "chat-nested-title-flow.json"
                | "chat-loop-flow.json"
                | "chat-calculator-flow.json"
                | "chat-context-flow.json"
                | "chat-deepseek-flow.json"
                | CODING_AGENT
        ) {
            let records: Vec<Value> = serde_json::Deserializer::from_slice(&output.stdout)
                .into_iter()
                .collect::<Result<_, _>>()
                .unwrap();
            let answer: String = records
                .iter()
                .filter_map(|record| record.get("answer"))
                .map(|delta| delta["text"].as_str().unwrap())
                .collect();
            assert_eq!(
                answer,
                if name == "chat-loop-flow.json" {
                    "replyreply"
                } else if name == "chat-deepseek-flow.json" {
                    "DeepSeek findings: https://example.com/release"
                } else if matches!(name, "chat-calculator-flow.json" | "chat-context-flow.json") {
                    CALCULATOR_ANSWER
                } else {
                    "reply"
                }
            );
            assert_eq!(server.as_ref().unwrap().requests()[0].body["stream"], true);
            if matches!(
                name,
                "chat-calculator-flow.json" | "chat-context-flow.json" | "chat-deepseek-flow.json"
            ) {
                let expected = if name == "chat-deepseek-flow.json" {
                    3
                } else {
                    2
                };
                let deltas: Vec<_> = records
                    .iter()
                    .filter_map(|record| record.get("tool_call_delta"))
                    .collect();
                assert_eq!(deltas.len(), expected, "{name}");
                let mut parts = std::collections::BTreeSet::new();
                for delta in deltas {
                    assert!(parts.insert((
                        delta["model_call_id"].as_str().unwrap(),
                        delta["index"].as_u64().unwrap()
                    )));
                    assert!(delta["id"].as_str().is_some());
                    assert!(
                        serde_json::from_str::<Value>(delta["arguments"].as_str().unwrap())
                            .unwrap()
                            .is_object()
                    );
                }
            }
            records
                .into_iter()
                .find(|record| {
                    record.get("append").is_some() || record.get("replace_context").is_some()
                })
                .unwrap()
        } else {
            serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{name}: invalid JSON output: {error}\n{}",
                    String::from_utf8_lossy(&output.stdout)
                )
            })
        };
        if name.starts_with("chat-") || name == "llm-tools.json" {
            let mut identities = std::collections::BTreeSet::new();
            let usages = if name == "chat-context-flow.json" {
                &mut actual["replace_context"]["usages"]
            } else if name.starts_with("chat-") {
                &mut actual["append"]["usages"]
            } else {
                &mut actual["usages"]
            };
            for usage in usages.as_array_mut().unwrap() {
                let id = usage["call_id"].as_str().unwrap().to_owned();
                assert!(!id.is_empty() && identities.insert(id));
                usage["call_id"] = "generated".into();
                assert!(usage["started_at"].as_i64().unwrap() > 0);
                usage["started_at"] = 0.into();
                assert!(usage.get("timing").is_none_or(Value::is_null));
            }
        }
        if name == CODING_AGENT {
            let root = &mut actual["append"]["messages"][0]["metadata"]["coding_workspace"];
            assert_eq!(*root, json!(temporary.path().join("chat-workspace")));
            *root = "$WORKSPACE".into();
        }
        assert_eq!(actual, expected, "{name}: unexpected workflow output");
        if name == "chat-calculator-flow.json" {
            let requests = server.as_ref().unwrap().requests();
            assert_eq!(requests.len(), 2);
            for request in &requests {
                assert_eq!(request.body["model"], "example-model");
                assert_eq!(request.body["stream"], true);
                assert!(request.headers.contains("Bearer synthetic-calculator-key"));
            }
            let initial_messages = json!([
                {"role":"system","content":[{"type":"text","text":"Use the calculator."}]},
                {"role":"user","content":"Earlier question"},
                {"role":"assistant","content":[{"type":"text","text":"Earlier answer"}]},
                {"role":"user","content":CALCULATOR_PROMPT}
            ]);
            assert_eq!(requests[0].body["messages"], initial_messages);
            let mut continued_messages = initial_messages.as_array().unwrap().clone();
            continued_messages.extend([
                json!({"role":"assistant","tool_calls":[
                    {"id":"call_add","type":"function","function":{"name":"calculator","arguments":"{\"left\":2.0,\"operation\":\"add\",\"right\":3.0}"}},
                    {"id":"call_multiply","type":"function","function":{"name":"calculator","arguments":"{\"left\":3.0,\"operation\":\"multiply\",\"right\":4.0}"}}
                ]}),
                json!({"role":"tool","tool_call_id":"call_add","content":"5.0"}),
                json!({"role":"tool","tool_call_id":"call_multiply","content":"12.0"}),
            ]);
            assert_eq!(requests[1].body["messages"], json!(continued_messages));
            assert_eq!(requests[1].body["tools"], requests[0].body["tools"]);
            assert_eq!(
                requests[0].body["tools"],
                json!([{
                    "type":"function",
                    "function":{
                        "name":"calculator",
                        "description":CALCULATOR_DESCRIPTION,
                        "parameters":{
                            "type":"object",
                            "properties":{
                                "left":{"type":"number"},
                                "operation":{"type":"string"},
                                "right":{"type":"number"}
                            },
                            "required":["left","operation","right"],
                            "additionalProperties":false
                        }
                    }
                }])
            );
        }
        if name == "chat-deepseek-flow.json" {
            let requests = server.as_ref().unwrap().requests();
            assert_eq!(requests.len(), 3);
            for request in &requests {
                assert_eq!(request.body["model"], "example-model");
                assert!(request.headers.contains("Bearer synthetic-llm-key"));
            }
            let tools = requests[0].body["tools"].as_array().unwrap();
            assert_eq!(tools.len(), 2);
            for (name, description_prefix, properties, required) in [
                (
                    "search",
                    "Search the web",
                    json!({"count":{"type":"integer"}, "query":{"type":"string"}}),
                    "query",
                ),
                (
                    "fetch",
                    "Fetch a web page",
                    json!({"url":{"type":"string"}}),
                    "url",
                ),
            ] {
                let function = &tools
                    .iter()
                    .find(|tool| tool["function"]["name"] == name)
                    .unwrap_or_else(|| panic!("missing tool {name}"))["function"];
                assert!(
                    function["description"]
                        .as_str()
                        .is_some_and(|text| text.starts_with(description_prefix)),
                    "unexpected description for {name}: {}",
                    function["description"]
                );
                assert_eq!(
                    function["parameters"],
                    json!({
                        "type":"object",
                        "properties":properties,
                        "required":[required],
                        "additionalProperties":false
                    })
                );
            }
            for request in &requests {
                let body = request.body.to_string();
                assert!(!body.contains("synthetic-deepseek-key"));
                assert!(!body.contains("synthetic-llm-key"));
                assert!(!body.contains(&deepseek_server.as_ref().unwrap().base_url));
            }
            let requests = deepseek_server.as_ref().unwrap().requests();
            assert_eq!(requests.len(), 3);
            let queries: std::collections::BTreeSet<_> = requests[..2]
                .iter()
                .map(|request| {
                    assert!(request.headers.starts_with("POST /prefix/v1/tools/search "));
                    assert!(request.headers.contains("Bearer synthetic-deepseek-key"));
                    assert_eq!(request.body["count"], 2);
                    request.body["query"].as_str().unwrap()
                })
                .collect();
            assert_eq!(
                queries,
                std::collections::BTreeSet::from(["DeepSeek Search API", "DeepSeek Fetch API"])
            );
            assert!(
                requests[2]
                    .headers
                    .starts_with("POST /prefix/v1/tools/fetch ")
            );
            assert!(
                requests[2]
                    .headers
                    .contains("Bearer synthetic-deepseek-key")
            );
            assert_eq!(
                requests[2].body,
                json!({"url":"https://example.com/release"})
            );
        }
        if name == "chat-context-flow.json" {
            let requests = server.as_ref().unwrap().requests();
            assert_eq!(requests.len(), 3);
            assert_ne!(requests[2].body["stream"], true);
            let summary = requests[2].body["messages"].as_array().unwrap();
            assert!(summary.iter().any(
                |message| message["tool_call_id"] == "call_add" && message["content"] == "5.0"
            ));
            assert!(
                summary
                    .iter()
                    .any(|message| message["tool_call_id"] == "call_multiply"
                        && message["content"] == "12.0")
            );
            assert!(
                summary.iter().any(|message| message["content"]
                    == json!([{"type":"text","text":CALCULATOR_ANSWER}]))
            );
        }
        if name == "exclusive-inputs.json" {
            for ci in [false, true] {
                let output = Command::new(env!("CARGO_BIN_EXE_mf"))
                    .args([
                        "run",
                        executable.to_str().unwrap(),
                        "--inputs",
                        &json!({"params.ci":ci}).to_string(),
                    ])
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert_eq!(
                    serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                    json!({"result":{"mode":if ci {"ci"} else {"local"}}})
                );
            }
        }
    }
}
