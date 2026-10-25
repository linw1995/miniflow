use mf_chat::{ChatHost, HostConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("provide a database path")?;
    let workflow = std::env::args_os()
        .nth(2)
        .ok_or("provide a conversation workflow JSON path")?;
    let definition = mf_compiler::load_workflow(std::path::Path::new(&workflow))?.definition;
    let profile = mf_chat::WorkflowProfile::new("default", definition, Default::default())?;
    let host = ChatHost::open(path, HostConfig::new(profile))?;
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
