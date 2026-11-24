use mfn_llm::test_support::Reply;
use serde_json::{Value, json};

pub const CODING_AGENT: &str = "chat-coding-flow.json";

/// Preserve source-relative package and Flow paths without writing lockfiles into the checkout.
#[cfg(unix)]
pub fn stage_example(root: &std::path::Path, name: &str) -> std::path::PathBuf {
    use std::{fs, os::unix::fs::symlink, path::Path};

    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let examples = crates.parent().unwrap().join("examples");
    let staged = root.join("examples");
    symlink(crates, root.join("crates")).unwrap();
    let children: &[&str] = match name {
        "chat-calculator-flow.json" => &["agent-calculator/calculator.json"],
        "chat-context-flow.json" => &[
            "chat-calculator-flow.json",
            "agent-calculator/calculator.json",
        ],
        "chat-deepseek-flow.json" => &["agent-deepseek/search.json", "agent-deepseek/fetch.json"],
        CODING_AGENT => &[
            "coding-read.json",
            "agent-coding/request.json",
            "agent-coding/mkdir.json",
            "agent-coding/ls.json",
            "agent-coding/find.json",
            "agent-coding/grep.json",
            "agent-coding/edit.json",
            "agent-coding/write.json",
            "agent-coding/bash.json",
        ],
        _ => &[],
    };
    for file in std::iter::once(name).chain(children.iter().copied()) {
        let target = staged.join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(examples.join(file), target).unwrap();
    }
    staged.join(name)
}

pub fn streamed_tool_calls(mut calls: Value) -> Reply {
    let mut streamed = Reply::stream_text("");
    for (index, call) in calls.as_array_mut().unwrap().iter_mut().enumerate() {
        call["index"] = json!(index);
    }
    let chunk = json!({"id":"tool-response","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":{"tool_calls":calls},"finish_reason":null}]});
    let finish = json!({"id":"tool-response","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}});
    streamed.body = format!("data: {chunk}\n\ndata: {finish}\n\ndata: [DONE]\n\n");
    streamed
}
