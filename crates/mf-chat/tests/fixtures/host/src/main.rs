use mf_chat::{ChatHost, HostConfig, ModelProfile, UsageQuery};
use std::time::{Duration, Instant};

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
    let mut config = HostConfig::new(profile()?);
    config.automatic_titles = false;
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
    host.database().schedule_titles(10)?;
    let call_a = host
        .database()
        .pending_titles(10)?
        .into_iter()
        .find(|call| call.session_id == a.id)
        .ok_or("title A is missing")?;
    host.start_title(&call_a.id)?.join()?;
    host.database().rename_session(&a.id, "Manual A")?;
    let call_b = host
        .database()
        .pending_titles(10)?
        .into_iter()
        .find(|call| call.session_id == b.id)
        .ok_or("title B is missing")?;
    host.close()?;
    let reopened = ChatHost::open(&path, HostConfig::new(profile()?))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(result) = reopened.poll_titles().pop() {
            let result = result?;
            if result.invocation.call_id != call_b.id {
                return Err("title identity changed after restart".into());
            }
            break;
        }
        if Instant::now() >= deadline {
            return Err("title execution did not complete".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
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
