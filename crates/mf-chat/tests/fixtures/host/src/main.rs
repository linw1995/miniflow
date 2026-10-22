use mf_chat::{ChatHost, HostConfig, ModelProfile, UsageQuery};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("provide a database path")?;
    let profile = || -> Result<ModelProfile, Box<dyn std::error::Error>> {
        let base_url = std::env::var("MF_CHAT_FIXTURE_URL")?;
        Ok(ModelProfile::new(
            "default",
            serde_json::json!({
                "model":"fixture-model", "base_url":base_url, "api_key_env":"MF_CHAT_FIXTURE_KEY",
            }),
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
            "history_b":reopened.database().history(&b.id, 1024)?.history.len(),
        })
    );
    reopened.close()?;
    Ok(())
}
