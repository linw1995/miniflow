fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    compiler::embed_workflows(&[
        compiler::EmbedRequest::new("answer", "answer.json"),
        compiler::EmbedRequest::new("events", "events.json"),
        compiler::EmbedRequest::new("contract", "contract.json"),
        compiler::EmbedRequest::new("contract_stream", "contract-stream.json"),
        compiler::EmbedRequest::new("typed", "typed.json"),
        compiler::EmbedRequest::new("counter", "counter.json"),
        compiler::EmbedRequest::new("repeat", "repeat.json"),
        compiler::EmbedRequest::new("iteration", "iteration.json"),
    ])?;
    Ok(())
}
