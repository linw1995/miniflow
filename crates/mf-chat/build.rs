fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=migrations");
    mf_compiler::embed_workflows(&[
        mf_compiler::EmbedRequest::new("conversation", "workflows/conversation.json"),
        mf_compiler::EmbedRequest::new("session_title", "workflows/session_title.json"),
    ])?;
    Ok(())
}
