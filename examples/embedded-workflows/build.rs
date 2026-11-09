fn main() -> Result<(), Box<dyn std::error::Error>> {
    mf_compiler::embed_workflows(&[mf_compiler::EmbedRequest::new("answer", "answer.json")
        .with_source("answer-apply.json")
        .with_source("answer-body.json")])?;
    Ok(())
}
