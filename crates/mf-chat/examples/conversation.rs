use mf_chat::{ChatHost, HostConfig, ModelProfile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("provide a database path")?;
    let model = std::env::var("MF_CHAT_MODEL")?;
    let mut config = serde_json::json!({ "model": model });
    if let Ok(base_url) = std::env::var("MF_CHAT_BASE_URL") {
        config["base_url"] = base_url.into();
    }
    let host = ChatHost::open(path, HostConfig::new(ModelProfile::new("default", config)?))?;
    let session = host
        .database()
        .create_session("default", Some("Answer briefly".into()), None)?;
    for prompt in ["Hello", "What did I just say?"] {
        let result = host.submit(&session.id, prompt)?.join()?;
        println!("{}", serde_json::to_string(&result)?);
    }
    host.close()?;
    Ok(())
}
