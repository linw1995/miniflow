use mf_chat::{ChatHost, HostConfig, UsageQuery, WorkflowProfile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("provide a database path")?;
    let profile = || -> Result<WorkflowProfile, Box<dyn std::error::Error>> {
        let base_url = std::env::var("MF_CHAT_FIXTURE_URL")?;
        let mut definition: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../../examples/chat-title-flow.json"
        ))?;
        let parameters = definition["nodes"][0]["config"]["parameters"]
            .as_object_mut()
            .unwrap();
        for field in ["model", "base_url", "api_key"] {
            parameters.remove(field);
        }
        for node in definition["nodes"].as_array_mut().unwrap() {
            if node["kind"] == "builtin.llm" {
                let attribution = node["config"]["attribution"].clone();
                node["config"] = serde_json::json!({
                    "model":"fixture-model", "base_url":base_url, "api_key_env":"MF_CHAT_FIXTURE_KEY",
                    "capture_errors":true,"attribution":attribution,
                });
            }
        }
        Ok(WorkflowProfile::new(
            "default",
            serde_json::from_value(definition)?,
            Default::default(),
        )?)
    };
    let config = HostConfig::new(profile()?);
    let host = ChatHost::open(&path, config)?;
    let a = host.database().create_session("default", None, None)?;
    let b = host.database().create_session("default", None, None)?;
    host.submit(&a.id, "first session")?.join()?;
    let failed = host.submit(&b.id, "retry this")?;
    let original = failed.invocation().clone();
    if failed.join().is_ok() {
        return Err("expected the synthetic provider failure".into());
    }
    host.retry(&original.turn_id)?.join()?;
    host.database().rename_session(&a.id, "Manual A")?;
    host.close()?;
    let reopened = ChatHost::open(&path, HostConfig::new(profile()?))?;
    let usage = reopened.usage(UsageQuery::default())?;
    println!(
        "{}",
        serde_json::json!({
            "usage":usage,
            "title_a":reopened.database().session(&a.id)?.title,
            "title_b":reopened.database().session(&b.id)?.title,
            "messages_a":reopened.database().messages(&a.id, 50, None)?.messages.len(),
            "messages_b":reopened.database().messages(&b.id, 50, None)?.messages.len(),
            "history_b":reopened.database().history(&b.id)?.history.len(),
        })
    );
    reopened.close()?;
    Ok(())
}
