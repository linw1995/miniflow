use super::*;

fn user(text: &str) -> Value {
    json!({"role": "user", "content": text})
}

fn tool() -> Value {
    json!({"name": "lookup", "description": "Look up a key", "input_schema": {
        "type": "object", "properties": {"key": {"type": "string"}},
        "required": ["key"], "additionalProperties": false
    }})
}

fn call(id: &str, key: &str) -> Value {
    json!({"id": id, "type": "function", "function": {
        "name": "lookup", "arguments": json!({"key": key}).to_string()
    }})
}

fn reply(text: Option<&str>, finish: &str, calls: Vec<Value>) -> Reply {
    Reply::json(json!({
        "id": "response", "object": "chat.completion", "created": 0, "model": "example-model",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": text, "tool_calls": calls}, "finish_reason": finish}],
        "usage": {"prompt_tokens": 9, "completion_tokens": 3, "total_tokens": 12}
    }))
}

fn invoke(server: &Server, input: Value, stream: bool, config: Value) -> Value {
    probe(server, json!({"inputs": [input], "stream": stream}), config)
}

#[test]
fn tools_are_advertised_and_multiple_calls_round_trip_without_an_extra_prompt() {
    let server = Server::start(vec![
        reply(
            None,
            "tool_calls",
            vec![call("first", "a"), call("second", "b")],
        ),
        Reply::text("Both values found"),
    ]);
    let input = json!({"messages":[{"role":"system","content":"Use the tools"},user("Look up a and b")], "tools":[tool()]});
    let results = json!({"messages": [], "tool_results": [
        {"tool_call_id":"second", "content":"B"},
        {"tool_call_id":"first", "content":"A"}
    ], "tools": [tool()]});
    let output = probe(
        &server,
        json!({"inputs": [input, results], "append_messages": true}),
        json!({}),
    );
    assert_eq!(output["results"][0]["ok"], true, "{output}");
    assert_eq!(output["results"][1]["ok"], true, "{output}");
    let first = &output["results"][0]["outputs"];
    assert_eq!(first["outcome"], "tool_calls");
    assert_eq!(first["text"], "");
    assert_eq!(first["tool_calls"].as_array().unwrap().len(), 2);
    assert_eq!(first["tool_calls"][0]["arguments"], json!({"key": "a"}));
    assert_eq!(first["message"]["tool_calls"][0], first["tool_calls"][0]);
    assert_eq!(first["usages"][0]["usage"]["total_tokens"], 12);
    assert_eq!(output["results"][1]["outputs"]["outcome"], "completed");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert!(request.body.get("tool_choice").is_none());
        assert_eq!(
            request.body["tools"][0]["function"]["parameters"],
            tool()["input_schema"]
        );
    }
    let sent = requests[1].body["messages"].as_array().unwrap();
    assert_eq!(sent.len(), 5, "{sent:?}");
    assert_eq!(sent[0]["role"], "system");
    assert_eq!(sent[2]["tool_calls"][0]["id"], "first");
    assert_eq!(sent[3]["role"], "tool");
    assert_eq!(sent[3]["tool_call_id"], "second");
    assert_eq!(sent[3]["content"], "B");
    assert_eq!(sent[4]["role"], "tool");
    assert_eq!(sent[4]["tool_call_id"], "first");
    assert_eq!(sent[4]["content"], "A");
}

#[test]
fn tool_results_preserve_content_without_readvertising_historical_tools() {
    let contents = [
        "Found the item",
        r#"{"value":5,"items":[1,true,null]}"#,
        r#"{"error":{"code":"not_found","message":"No matching item"}}"#,
        "",
    ];
    let calls: Vec<_> = contents
        .iter()
        .enumerate()
        .map(|(index, _)| json!({"id":format!("call_{index}"),"name":"lookup","arguments":{"key":index.to_string()}}))
        .collect();
    let mut messages = vec![
        user("Look up the items"),
        json!({"role":"assistant","content":"Checking","tool_calls":calls}),
    ];
    messages.extend(contents.iter().enumerate().rev().map(|(index, content)| {
        json!({"role":"tool","tool_call_id":format!("call_{index}"),"content":content,
            "metadata":{"local":true,"tags":["tool-result"]}})
    }));
    for result_start in [2, 4, 6] {
        for stream in [false, true] {
            let reply = if stream {
                Reply::stream_text("Results received")
            } else {
                Reply::text("Results received")
            };
            let server = Server::start(vec![reply]);
            let results: Vec<_> = messages[result_start..].iter().map(|message| {
            json!({"tool_call_id":message["tool_call_id"],"content":message["content"]})
        }).collect();
            let input = json!({"messages":messages[..result_start],"tool_results":results});
            let output = invoke(&server, input.clone(), stream, json!({}));
            assert_eq!(output["results"][0]["ok"], true, "{output}");
            assert_eq!(output["results"][0]["outputs"]["outcome"], "completed");
            assert_eq!(output["results"][0]["outputs"]["text"], "Results received");
            assert_eq!(output["original_inputs"][0], input);
            let requests = server.requests();
            assert_eq!(requests.len(), 1);
            let body = &requests[0].body;
            assert!(
                body.get("tools")
                    .is_none_or(|tools| tools.as_array().is_some_and(Vec::is_empty)),
                "{body}"
            );
            let sent = body["messages"].as_array().unwrap();
            assert_eq!(sent.len(), messages.len());
            assert_eq!(sent[0], messages[0]);
            assert_eq!(
                sent[1]["content"],
                json!([{"type":"text","text":"Checking"}])
            );
            for (index, call) in sent[1]["tool_calls"].as_array().unwrap().iter().enumerate() {
                assert_eq!(call["id"], format!("call_{index}"));
                assert_eq!(call["type"], "function");
                assert_eq!(call["function"]["name"], "lookup");
                assert_eq!(
                    serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap())
                        .unwrap(),
                    json!({"key":index.to_string()})
                );
            }
            for (sent, supplied) in sent[2..].iter().zip(&messages[2..]) {
                assert_eq!(sent["role"], "tool");
                assert_eq!(sent["tool_call_id"], supplied["tool_call_id"]);
                assert_eq!(sent["content"], supplied["content"]);
                assert!(sent.get("metadata").is_none());
            }
        }
    }
}

#[test]
fn invalid_explicit_tool_results_fail_with_indexed_errors_before_http() {
    let pending = json!({"role":"assistant","content":"","tool_calls":[
        {"id":"one","name":"lookup","arguments":{}}
    ]});
    let result = json!({"tool_call_id":"one","content":"value"});
    let server = Server::start(vec![]);
    for (messages, results, path) in [
        (json!([pending.clone()]), json!(null), "/tool_results"),
        (json!([pending.clone()]), json!({}), "/tool_results"),
        (
            json!([pending.clone()]),
            json!([{"content":"value"}]),
            "/tool_results/0/tool_call_id",
        ),
        (
            json!([pending.clone()]),
            json!([{"tool_call_id":"","content":"value"}]),
            "/tool_results/0/tool_call_id",
        ),
        (
            json!([pending.clone()]),
            json!([{"tool_call_id":"unknown","content":"value"}]),
            "/tool_results/0/tool_call_id",
        ),
        (
            json!([pending.clone()]),
            json!([{"tool_call_id":"one"}]),
            "/tool_results/0/content",
        ),
        (
            json!([pending.clone()]),
            json!([{"tool_call_id":"one","content":{"value":5}}]),
            "/tool_results/0/content",
        ),
        (
            json!([pending.clone()]),
            json!([{"tool_call_id":"one","content":"value","role":"tool"}]),
            "/tool_results/0/role",
        ),
        (
            json!([pending.clone()]),
            json!([result.clone(), result.clone()]),
            "/tool_results/1/tool_call_id",
        ),
        (
            json!([pending.clone(),{"role":"tool","tool_call_id":"one","content":"value"}]),
            json!([result.clone()]),
            "/tool_results/0/tool_call_id",
        ),
        (
            json!([user("Hi")]),
            json!([result.clone()]),
            "/tool_results/0/tool_call_id",
        ),
        (
            json!([pending.clone(), user("Hi")]),
            json!([result.clone()]),
            "/messages/1",
        ),
        (
            json!([{"role":"assistant","content":"","tool_calls":[
                {"id":"one","name":"lookup","arguments":{}},
                {"id":"two","name":"lookup","arguments":{}}
            ]}]),
            json!([result]),
            "/tool_results",
        ),
    ] {
        for stream in [false, true] {
            let input = json!({"messages":messages,"tool_results":results});
            let output = invoke(
                &server,
                input.clone(),
                stream,
                json!({"capture_errors":true}),
            );
            let failure = &output["results"][0];
            assert_eq!(failure["ok"], false, "{input}: {output}");
            assert!(
                failure["error"].as_str().unwrap().contains(path),
                "{input}: {output}"
            );
            assert_eq!(output["original_inputs"][0], input);
        }
    }
    assert!(server.requests().is_empty());
}

#[test]
fn fragmented_interleaved_streamed_calls_match_the_terminal_response() {
    let mut streamed = Reply::stream_text("");
    let chunks = [
        json!({"content":"Checking"}),
        json!({"tool_calls":[
            {"index":0,"id":"first","type":"function","function":{"name":"lookup","arguments":"{\"key\":"}},
            {"index":1,"id":"second","type":"function","function":{"name":"lookup","arguments":"{\"key\":"}}
        ]}),
        json!({"tool_calls":[{"index":1,"function":{"arguments":"\"b\"}"}}]}),
        json!({"tool_calls":[{"index":0,"function":{"arguments":"\"a\"}"}}]}),
    ];
    streamed.body = chunks.into_iter().map(|delta| {
        let chunk = json!({"id":"response","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":delta,"finish_reason":null}]});
        format!("data: {chunk}\n\n")
    }).collect();
    let finish = json!({"id":"response","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":9,"completion_tokens":3,"total_tokens":12}});
    streamed
        .body
        .push_str(&format!("data: {finish}\n\ndata: [DONE]\n\n"));
    streamed.fragments = Some(
        streamed
            .body
            .as_bytes()
            .chunks(7)
            .map(<[u8]>::to_vec)
            .collect(),
    );
    let server = Server::start(vec![
        reply(
            Some("Checking"),
            "tool_calls",
            vec![call("first", "a"), call("second", "b")],
        ),
        streamed,
    ]);
    let input = json!({"messages": [user("Look up a and b")], "tools": [tool()]});
    let terminal = invoke(&server, input.clone(), false, json!({}));
    let streamed = invoke(&server, input, true, json!({}));
    assert_eq!(terminal["results"][0]["ok"], true, "{terminal}");
    assert_eq!(streamed["results"][0]["ok"], true, "{streamed}");
    let mut left = terminal["results"][0]["outputs"].clone();
    let mut right = streamed["results"][0]["outputs"].clone();
    for result in [&mut left, &mut right] {
        let usage = result["usages"][0].as_object_mut().unwrap();
        usage.remove("call_id");
        usage.remove("started_at");
    }
    assert_eq!(left, right);
    assert_eq!(
        streamed["results"][0]["deltas"][0]["delta"]["text"],
        "Checking"
    );
    assert_eq!(
        streamed["results"][0]["events"].as_array().unwrap().len(),
        3
    );
    let records = streamed["results"][0]["deltas"].as_array().unwrap();
    let model_call_id = &streamed["results"][0]["outputs"]["usages"][0]["call_id"];
    let mut calls = Vec::new();
    for record in records {
        if let Some(event) = record.get("tool_call") {
            assert!(record.get("delta").is_none());
            assert_eq!(&event["model_call_id"], model_call_id);
            calls.push(event["call"].clone());
        }
    }
    calls.sort_by_key(|call| call["id"].as_str().unwrap().to_owned());
    assert_eq!(
        json!(calls),
        streamed["results"][0]["outputs"]["tool_calls"]
    );
}

fn streamed_tool(arguments: &str, finish_reason: &str) -> Reply {
    let mut reply = Reply::stream_text("");
    let chunk = json!({"id":"response","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"one","type":"function","function":{"name":"lookup","arguments":arguments}}]},"finish_reason":null}]});
    let finish = json!({"id":"response","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":{},"finish_reason":finish_reason}],"usage":{"prompt_tokens":9,"completion_tokens":3,"total_tokens":12}});
    reply.body = format!("data: {chunk}\n\ndata: {finish}\n\ndata: [DONE]\n\n");
    reply
}

#[test]
fn tool_only_streams_publish_calls_without_text_or_first_text_observations() {
    let server = Server::start(vec![streamed_tool(r#"{"key":"a"}"#, "tool_calls")]);
    let output = invoke(
        &server,
        json!({"messages": [user("Look up a")], "tools":[tool()]}),
        true,
        json!({}),
    );
    let result = &output["results"][0];
    assert_eq!(result["ok"], true, "{output}");
    assert_eq!(result["outputs"]["outcome"], "tool_calls");
    let deltas = result["deltas"].as_array().unwrap();
    assert!(deltas.iter().all(|delta| delta.get("delta").is_none()));
    assert_eq!(deltas.len(), 1);
    assert_eq!(
        deltas.last().unwrap()["tool_call"]["call"],
        result["outputs"]["tool_calls"][0]
    );
    assert_eq!(result["events"].as_array().unwrap().len(), 2);
}

#[test]
fn unsolicited_tool_streams_retain_the_text_only_failure_contract() {
    let server = Server::start(vec![streamed_tool("{}", "tool_calls")]);
    let output = invoke(
        &server,
        json!({"messages": [user("Hello")]}),
        true,
        json!({"capture_errors":true}),
    );
    let result = &output["results"][0];
    assert_eq!(result["ok"], true, "{output}");
    assert_eq!(result["outputs"]["outcome"], "failed");
    assert_eq!(result["deltas"], json!([]));
}

#[test]
fn invalid_or_truncated_tool_streams_do_not_publish_completed_calls() {
    for (finish, expected) in [("tool_calls", "failed"), ("length", "truncated")] {
        let server = Server::start(vec![streamed_tool(r#"{"key":"#, finish)]);
        let output = invoke(
            &server,
            json!({"messages": [user("Look up a")], "tools":[tool()]}),
            true,
            json!({"capture_errors":true}),
        );
        let result = &output["results"][0];
        assert_eq!(result["ok"], true, "{output}");
        assert_eq!(result["outputs"]["outcome"], expected);
        assert_eq!(result["outputs"]["tool_calls"], json!([]));
        assert!(result["outputs"].get("message").is_none());
        let deltas = result["deltas"].as_array().unwrap();
        assert!(deltas.is_empty(), "{output}");
    }
}

#[test]
fn tool_stream_delivery_failure_and_cancellation_remain_fatal() {
    for option in ["disconnect", "cancel_after_prefix"] {
        let server = Server::start(vec![streamed_tool("{}", "tool_calls")]);
        let mut input =
            json!({"stream":true, "inputs":[{"messages": [user("Look up a")], "tools":[tool()]}]});
        input[option] = true.into();
        let output = probe(&server, input, json!({"capture_errors":true}));
        let result = &output["results"][0];
        assert_eq!(result["ok"], false, "{output}");
        assert_eq!(result["deltas"].as_array().unwrap().len(), 1);
        assert_eq!(result["deltas"][0]["tool_call"]["call"]["id"], "one");
        if option == "disconnect" {
            assert!(result["error"].as_str().unwrap().contains("publish"));
        } else {
            assert_eq!(result["typed_runtime"], true, "{output}");
        }
    }
}

#[test]
fn text_calls_need_no_tools_and_truncation_is_not_completion() {
    for (finish, expected) in [
        ("stop", "completed"),
        ("length", "truncated"),
        ("content_filter", "refused"),
        ("custom_stop", "incomplete"),
    ] {
        let server = Server::start(vec![reply(Some("Answer"), finish, vec![])]);
        let output = invoke(
            &server,
            json!({"messages": [user("Hello")]}),
            false,
            json!({}),
        );
        assert_eq!(output["results"][0]["ok"], true, "{output}");
        let result = &output["results"][0]["outputs"];
        assert_eq!(result["outcome"], expected);
        assert_eq!(result["tool_calls"], json!([]));
        assert_eq!(result["text"], "Answer");
    }
}

#[test]
fn invalid_tool_definitions_and_unpaired_histories_fail_before_http() {
    let pending = json!({"role":"assistant", "content":"", "tool_calls":[{"id":"one", "name":"lookup", "arguments":{}}]});
    let result = json!({"role":"tool", "tool_call_id":"one", "content":"value"});
    let cases = [
        json!({"messages":[]}),
        json!({"messages":[user("Hi")], "tool_choice":"auto"}),
        json!({"messages":[user("Hi")], "tools":[tool(),tool()]}),
        json!({"messages":[user("Hi")], "tools":[{"name":"bad name","description":"x","input_schema":{"type":"object"}}]}),
        json!({"messages":[user("Hi")], "tools":[{"name":"valid","description":"x","input_schema":false}]}),
        json!({"messages":[pending.clone()]}),
        json!({"messages":[result.clone()]}),
        json!({"messages":[pending.clone(),user("Hi"),result.clone()]}),
        json!({"messages":[pending.clone(),result.clone(),result.clone()]}),
        json!({"messages":[pending.clone(),result.clone(),pending.clone(),result.clone()]}),
        json!({"messages":[pending.clone(),{"role":"tool","tool_call_id":"unknown","content":"value"}]}),
        json!({"messages":[pending.clone(),{"role":"tool","content":"value"}]}),
        json!({"messages":[pending.clone(),{"role":"tool","tool_call_id":"","content":"value"}]}),
        json!({"messages":[pending.clone(),{"role":"tool","tool_call_id":"one","content":{"value":5}}]}),
        json!({"messages":[pending.clone(),{"role":"tool","tool_call_id":"one","content":"value","tool_calls":[]}]}),
        json!({"messages":[pending.clone(),{"role":"system","content":"Continue"},result.clone()]}),
        json!({"messages":[{"role":"assistant","content":"","tool_calls":[
            {"id":"one","name":"lookup","arguments":{}},
            {"id":"two","name":"lookup","arguments":{}}
        ]},result]}),
        json!({"messages":[{"role":"assistant","tool_call_id":"one","content":"value"}]}),
        json!({"messages":[{"role":"user","content":"Hi","tool_calls":[{"id":"one","name":"lookup","arguments":{}}]}]}),
    ];
    let server = Server::start(vec![]);
    for input in cases {
        let output = invoke(
            &server,
            input.clone(),
            false,
            json!({"capture_errors":true}),
        );
        assert_eq!(output["results"][0]["ok"], false, "{input}: {output}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn captured_provider_failures_publish_no_message_or_transcript() {
    let mut failure = Reply::json(
        json!({"error":{"message":"private provider detail"}, "usage":{"prompt_tokens":9}}),
    );
    failure.status = 500;
    let server = Server::start(vec![failure]);
    let output = invoke(
        &server,
        json!({"messages":[user("Hi")]}),
        false,
        json!({"capture_errors":true}),
    );
    assert_eq!(output["results"][0]["ok"], true, "{output}");
    let result = &output["results"][0]["outputs"];
    assert_eq!(result["outcome"], "failed");
    assert!(result.get("message").is_none());
    assert!(result.get("history").is_none());
    assert_eq!(
        output["original_inputs"][0]["messages"],
        json!([user("Hi")])
    );
    assert_eq!(result["tool_calls"], json!([]));
    assert_eq!(result["usages"].as_array().unwrap().len(), 1);
    assert!(!result["usages"][0]["call_id"].as_str().unwrap().is_empty());
    assert!(
        !result["error"]
            .as_str()
            .unwrap()
            .contains("private provider detail")
    );
}

#[test]
fn malformed_tool_arguments_fail_instead_of_publishing_executable_calls() {
    let mut invalid = call("one", "a");
    invalid["function"]["arguments"] = json!("{broken");
    let server = Server::start(vec![reply(None, "tool_calls", vec![invalid])]);
    let output = invoke(
        &server,
        json!({"messages":[user("Hi")], "tools":[tool()]}),
        false,
        json!({"capture_errors":true}),
    );
    assert_eq!(output["results"][0]["ok"], true, "{output}");
    assert_eq!(output["results"][0]["outputs"]["outcome"], "failed");
    assert_eq!(output["results"][0]["outputs"]["tool_calls"], json!([]));
}

#[test]
fn stream_publication_failure_is_not_captured_as_a_model_failure() {
    let server = Server::start(vec![Reply::stream_text("prefix")]);
    let output = probe(
        &server,
        json!({"stream":true, "disconnect":true, "inputs":[{"messages":[user("Hi")]}]}),
        json!({"capture_errors":true}),
    );
    assert_eq!(output["results"][0]["ok"], false, "{output}");
    assert!(
        output["results"][0]["error"]
            .as_str()
            .unwrap()
            .contains("publish")
    );
}
