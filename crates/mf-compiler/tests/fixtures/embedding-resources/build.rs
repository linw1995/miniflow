fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MF_EMBED_RESOURCE_EXTRA");
    let mut answer = compiler::EmbedRequest::new("answer", "flows/entry.json")
        .with_source("flows/children/child.json")
        .with_source("flows/children/nested/leaf.json")
        .with_source("unused-malformed.json");
    if let Some(source) = std::env::var_os("MF_EMBED_RESOURCE_EXTRA") {
        answer = answer.with_source(source);
    }
    compiler::embed_workflows(&[
        answer,
        compiler::EmbedRequest::new("repeat", "repeat.json").with_source("flows/loop-body.json"),
        compiler::EmbedRequest::new("iteration", "iteration.json")
            .with_source("flows/iteration-body.json"),
        // This entry deliberately omits a source that exists on the author's filesystem.
        compiler::EmbedRequest::new("missing", "missing.json"),
    ])?;
    Ok(())
}
