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
    let input = json!({"prompt":"Look up a and b", "history":[], "tools": [tool()], "system": "Use the tools"});
    let results = json!({"history": [
        {"role":"tool", "tool_call_id":"second", "content":"B"},
        {"role":"tool", "tool_call_id":"first", "content":"A"}
    ], "tools": [tool()], "system":"Use the tools"});
    let output = probe(
        &server,
        json!({"inputs": [input, results], "append_history": true}),
        json!({}),
    );
    assert_eq!(output["results"][0]["ok"], true, "{output}");
    assert_eq!(output["results"][1]["ok"], true, "{output}");
    let first = &output["results"][0]["outputs"];
    assert_eq!(first["outcome"], "tool_calls");
    assert_eq!(first["text"], "");
    assert_eq!(first["tool_calls"].as_array().unwrap().len(), 2);
    assert_eq!(first["tool_calls"][0]["arguments"], json!({"key": "a"}));
    assert_eq!(first["history"][1]["tool_calls"][0], first["tool_calls"][0]);
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
    assert_eq!(sent[4]["tool_call_id"], "first");
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
    let input = json!({"history": [user("Look up a and b")], "tools": [tool()]});
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
            json!({"history": [user("Hello")]}),
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
        json!({"history":[]}),
        json!({"history":[user("Hi")], "tool_choice":"auto"}),
        json!({"history":[user("Hi")], "tools":[tool(),tool()]}),
        json!({"history":[user("Hi")], "tools":[{"name":"bad name","description":"x","input_schema":{"type":"object"}}]}),
        json!({"history":[user("Hi")], "tools":[{"name":"valid","description":"x","input_schema":false}]}),
        json!({"history":[pending.clone()]}),
        json!({"history":[result.clone()]}),
        json!({"history":[pending.clone(),user("Hi"),result.clone()]}),
        json!({"history":[pending.clone(),result.clone(),result.clone()]}),
        json!({"history":[pending.clone(),result.clone(),pending.clone(),result]}),
        json!({"history":[{"role":"assistant","tool_call_id":"one","content":"value"}]}),
        json!({"history":[{"role":"user","content":"Hi","tool_calls":[{"id":"one","name":"lookup","arguments":{}}]}]}),
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
fn captured_provider_failures_keep_history_and_call_identity() {
    let mut failure = Reply::json(
        json!({"error":{"message":"private provider detail"}, "usage":{"prompt_tokens":9}}),
    );
    failure.status = 500;
    let server = Server::start(vec![failure]);
    let output = invoke(
        &server,
        json!({"history":[user("Hi")]}),
        false,
        json!({"capture_errors":true}),
    );
    assert_eq!(output["results"][0]["ok"], true, "{output}");
    let result = &output["results"][0]["outputs"];
    assert_eq!(result["outcome"], "failed");
    assert_eq!(result["history"], json!([user("Hi")]));
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
        json!({"history":[user("Hi")], "tools":[tool()]}),
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
        json!({"stream":true, "disconnect":true, "inputs":[{"history":[user("Hi")]}]}),
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
