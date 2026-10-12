fn main() -> Result<(), Box<dyn std::error::Error>> {
    mf_compiler::embed_workflows(&[mf_compiler::EmbedRequest::new("answer", "answer.json")])?;
    Ok(())
}
