use crate::WorkflowDefinition;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::BTreeSet,
    env, fs, io,
    path::{Path, PathBuf},
};

/// A named workflow definition to include in the host program.
#[derive(Clone, Debug)]
pub struct EmbedRequest {
    pub name: String,
    pub definition: PathBuf,
}

impl EmbedRequest {
    pub fn new(name: impl Into<String>, definition: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            definition: definition.into(),
        }
    }
}

#[derive(Debug, Snafu)]
pub enum EmbeddingError {
    #[snafu(display("could not resolve embedded workflow `{name}` at {path:?}: {source}"))]
    Sources {
        name: String,
        path: PathBuf,
        source: crate::FlowResolutionError,
    },
    #[snafu(display("workflow embedding requires Cargo environment variable {variable}"))]
    CargoEnvironment { variable: &'static str },
    #[snafu(display("embedded workflow name `{name}` must be a portable Rust module identifier"))]
    InvalidName { name: String },
    #[snafu(display("embedded workflow name `{name}` occurs more than once in the request batch"))]
    DuplicateName { name: String },
    #[snafu(display("could not read embedded workflow `{name}` at {path:?}: {source}"))]
    Read {
        name: String,
        path: PathBuf,
        source: io::Error,
    },
    #[snafu(display("could not parse embedded workflow `{name}` at {path:?}: {source}"))]
    Definition {
        name: String,
        path: PathBuf,
        source: crate::DefinitionParseError,
    },
    #[snafu(display("could not plan embedded workflow `{name}` at {path:?}: {source}"))]
    Planning {
        name: String,
        path: PathBuf,
        #[snafu(source(from(crate::WorkflowCompileError, Box::new)))]
        source: Box<crate::WorkflowCompileError>,
    },
    #[snafu(display("could not generate embedded workflow `{name}` at {path:?}: {source}"))]
    Generation {
        name: String,
        path: PathBuf,
        #[snafu(source(from(crate::PlanError, Box::new)))]
        source: Box<crate::PlanError>,
    },
    #[snafu(display("embedding output directory {path:?} must not be a symbolic link"))]
    LinkedOutput { path: PathBuf },
    #[snafu(display("could not create embedding output directory {path:?}: {source}"))]
    Directory { path: PathBuf, source: io::Error },
    #[snafu(transparent)]
    State { source: crate::StateError },
}

/// Check workflow structure and emit source modules into Cargo's OUT_DIR.
///
/// Paths are relative to CARGO_MANIFEST_DIR. The host links node crates as normal dependencies;
/// preparation validates their configuration, ports, and execution contracts at runtime.
pub fn embed_workflows(requests: &[EmbedRequest]) -> Result<(), EmbeddingError> {
    let manifest = env::var_os("CARGO_MANIFEST_DIR").context(CargoEnvironmentSnafu {
        variable: "CARGO_MANIFEST_DIR",
    })?;
    let output = env::var_os("OUT_DIR").context(CargoEnvironmentSnafu {
        variable: "OUT_DIR",
    })?;
    embed_workflows_at(requests, Path::new(&manifest), Path::new(&output))
}

fn portable_identifier(name: &str) -> bool {
    name != "_"
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && syn::parse_str::<syn::Ident>(name).is_ok()
}

fn create_output_directory(path: &Path) -> Result<(), EmbeddingError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            !metadata.file_type().is_symlink(),
            LinkedOutputSnafu { path }
        ),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(source).context(DirectorySnafu { path }),
    }
    fs::create_dir_all(path).context(DirectorySnafu { path })
}

fn embed_workflows_at(
    requests: &[EmbedRequest],
    manifest: &Path,
    output: &Path,
) -> Result<(), EmbeddingError> {
    let mut names = BTreeSet::new();
    for request in requests {
        ensure!(
            portable_identifier(&request.name),
            InvalidNameSnafu {
                name: &request.name
            }
        );
        ensure!(
            names.insert(&request.name),
            DuplicateNameSnafu {
                name: &request.name
            }
        );
    }
    let mut modules = Vec::new();
    for request in requests {
        let name = &request.name;
        let path = manifest.join(&request.definition);
        println!("cargo:rerun-if-changed={}", path.display());
        let source = fs::read_to_string(&path).context(ReadSnafu { name, path: &path })?;
        let definition = WorkflowDefinition::from_json(&source)
            .context(DefinitionSnafu { name, path: &path })?;
        let canonical = fs::canonicalize(&path).context(ReadSnafu { name, path: &path })?;
        println!("cargo:rerun-if-changed={}", canonical.display());
        let resolved = crate::flow_sources::resolve_embedded_flows(
            &definition,
            canonical.parent().expect("canonical file has a parent"),
        )
        .context(SourcesSnafu { name, path: &path })?;
        for source in resolved.sources {
            println!("cargo:rerun-if-changed={}", source.display());
        }
        let plan = crate::compiler::plan_resolved_definition(&resolved.definition)
            .context(PlanningSnafu { name, path: &path })?;
        let module = plan
            .generate_embedded_artifacts()
            .context(GenerationSnafu { name, path: &path })?;
        modules.push((output.join("mf-embedded").join(name), module.rust_source));
    }
    // A structural failure must preserve every module from the previous successful build.
    if modules.is_empty() {
        return Ok(());
    }
    create_output_directory(&output.join("mf-embedded"))?;
    for (directory, _) in &modules {
        create_output_directory(directory)?;
    }
    for (directory, source) in modules {
        crate::write_if_changed(&directory.join("workflow.rs"), source.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate mfn_code as _;
    extern crate mfn_core as _;
    extern crate mfn_llm as _;

    use super::*;
    use serde_json::json;

    fn definition() -> serde_json::Value {
        json!({
            "version": "2026-10-03", "dependencies": {},
            "nodes": [
                {"id": "source", "kind": "unlinked.source"},
                {"id": "echo", "kind": "unlinked.echo"}
            ],
            "edges": [{"from_node": "source", "from_output": "value", "to_node": "echo", "to_input": "input"}],
            "outputs": [{"name": "answer", "node": "echo", "port": "value"}]
        })
    }

    fn request() -> EmbedRequest {
        EmbedRequest::new("answer", "flow.json")
    }

    #[test]
    fn nested_sources_preserve_host_owned_provider_dependencies() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("flows")).unwrap();
        let mut child = definition();
        child["version"] = json!("2026-10-25");
        child["dependencies"] =
            json!({"provider":{"package":"host-provider","path":"unused-by-host"}});
        let parent = json!({
            "version":"2026-10-25", "dependencies":{"provider":{"package":"host-provider","path":"also-unused"}},
            "flows":{"child":{"path":"flows/child.json"}},
            "nodes":[{"id":"invoke","kind":"workflow.call","config":{"flow":"child"}}],
            "outputs":[{"name":"answer","node":"invoke","port":"answer"}]
        });
        // The child fixture uses the legacy edge spelling, so encode its modern authoring form.
        child["nodes"][1]["inputs"] = json!({"input":{"from":{"node":"source","port":"value"}}});
        child["nodes"][0]["id"] = json!("source");
        child["nodes"][1]["id"] = json!("echo");
        child["edges"] = json!([]);
        child["outputs"] = json!([{"name":"answer","node":"echo","port":"value"}]);
        fs::write(root.path().join("flows/child.json"), child.to_string()).unwrap();
        write_definition(root.path(), &parent);
        let output = root.path().join("out");
        embed_workflows_at(&[request()], root.path(), &output).unwrap();
        let module = fs::read_to_string(output.join("mf-embedded/answer/workflow.rs")).unwrap();
        assert!(module.contains("workflow.call"));
        assert!(module.contains("unlinked.echo"));
        assert!(!module.contains("\"path\":\"flows/child.json\""));
    }

    fn write_definition(root: &Path, value: &serde_json::Value) {
        fs::write(root.join("flow.json"), value.to_string()).unwrap();
    }

    #[test]
    fn embedded_artifacts_keep_injected_flow_bodies() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let output = tempfile::tempdir().unwrap();
        embed_workflows_at(
            &[EmbedRequest::new(
                "agent",
                examples.join("chat-calculator-flow.json"),
            )],
            &examples,
            output.path(),
        )
        .unwrap();
        let module =
            fs::read_to_string(output.path().join("mf-embedded/agent/workflow.rs")).unwrap();
        assert!(module.contains("compiler.metadata_expansion"), "{module}");
        assert!(module.contains(r#"\"calculator\""#), "{module}");
        assert!(module.contains("pub fn finalized_plan"), "{module}");
    }

    #[test]
    fn requires_cargo_environment_and_resolves_manifest_relative_inputs() {
        if let Ok(case) = env::var("MF_EMBEDDING_ENV_CASE") {
            let requests = [request()];
            let result = embed_workflows(if case == "empty" { &[] } else { &requests });
            match case.as_str() {
                "manifest" => assert!(matches!(
                    result,
                    Err(EmbeddingError::CargoEnvironment {
                        variable: "CARGO_MANIFEST_DIR"
                    })
                )),
                "output" => assert!(matches!(
                    result,
                    Err(EmbeddingError::CargoEnvironment {
                        variable: "OUT_DIR"
                    })
                )),
                _ => result.unwrap(),
            }
            return;
        }
        // Cargo environment contracts are process-local; avoid mutating the parallel test process.
        let root = tempfile::tempdir().unwrap();
        write_definition(root.path(), &definition());
        let working_directory = root.path().join("cwd");
        fs::create_dir(&working_directory).unwrap();
        for case in ["manifest", "output", "empty", "generate"] {
            let directory = root.path().join(case);
            let mut command = std::process::Command::new(env::current_exe().unwrap());
            command
                .args(["--exact", "embedding::tests::requires_cargo_environment_and_resolves_manifest_relative_inputs"])
                .current_dir(&working_directory)
                .env("MF_EMBEDDING_ENV_CASE", case)
                .env("CARGO_MANIFEST_DIR", root.path())
                .env("OUT_DIR", &directory);
            if case == "manifest" {
                command.env_remove("CARGO_MANIFEST_DIR");
            } else if case == "output" {
                command.env_remove("OUT_DIR");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{case}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                directory.join("mf-embedded/answer/workflow.rs").is_file(),
                case == "generate"
            );
        }
    }

    #[test]
    fn rejects_invalid_requests_before_replacing_existing_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("out");
        write_definition(root.path(), &definition());
        let file = output.join("mf-embedded/answer/workflow.rs");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "sentinel").unwrap();
        for name in [
            "",
            "_",
            "fn",
            "self",
            "r#answer",
            "../answer",
            "9answer",
            "answer-name",
            "caf\u{e9}",
        ] {
            let error = embed_workflows_at(
                &[EmbedRequest::new(name, "flow.json")],
                root.path(),
                &output,
            )
            .unwrap_err();
            assert!(
                matches!(error, EmbeddingError::InvalidName { .. }),
                "{error}"
            );
        }
        let error = embed_workflows_at(&[request(), request()], root.path(), &output).unwrap_err();
        assert!(matches!(error, EmbeddingError::DuplicateName { .. }));
        let mut invalid = definition();
        fs::write(root.path().join("valid.json"), invalid.to_string()).unwrap();
        invalid["edges"][0]["to_node"] = json!("source");
        write_definition(root.path(), &invalid);
        let error = embed_workflows_at(
            &[
                EmbedRequest::new("answer", "valid.json"),
                EmbedRequest::new("other", "flow.json"),
            ],
            root.path(),
            &output,
        )
        .unwrap_err();
        assert!(matches!(error, EmbeddingError::Planning { .. }));
        assert_eq!(fs::read_to_string(file).unwrap(), "sentinel");
    }

    #[test]
    fn preserves_definition_and_planning_error_sources() {
        use std::error::Error;
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("out");
        fs::write(root.path().join("flow.json"), "{").unwrap();
        let error = embed_workflows_at(&[request()], root.path(), &output).unwrap_err();
        assert!(error.source().unwrap().is::<crate::DefinitionParseError>());
        let mut value = definition();
        value["edges"][0]["to_node"] = json!("source");
        write_definition(root.path(), &value);
        let error = embed_workflows_at(&[request()], root.path(), &output).unwrap_err();
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<Box<crate::WorkflowCompileError>>()
            .unwrap();
        assert!(matches!(
            source.as_ref(),
            crate::WorkflowCompileError::Cycle { .. }
        ));
        assert!(!output.exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_output_directory_links() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        write_definition(root.path(), &definition());
        let outside = root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        for relative in ["mf-embedded", "mf-embedded/answer"] {
            let output = tempfile::tempdir().unwrap();
            let link = output.path().join(relative);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(&outside, &link).unwrap();
            let error = embed_workflows_at(&[request()], root.path(), output.path()).unwrap_err();
            assert!(matches!(error, EmbeddingError::LinkedOutput { .. }));
        }
        assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
    }
}
