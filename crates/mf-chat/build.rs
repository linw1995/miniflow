fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=migrations");
    mf_compiler::embed_workflows(&[mf_compiler::EmbedRequest::new(
        "conversation",
        "workflows/conversation.json",
    )])?;
    Ok(())
}
