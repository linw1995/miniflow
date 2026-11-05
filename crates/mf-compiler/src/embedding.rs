use crate::WorkflowDefinition;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs, io,
    path::{Path, PathBuf},
};

/// A named workflow definition to include in the host program.
#[derive(Clone, Debug)]
pub struct EmbedRequest {
    pub name: String,
    pub definition: PathBuf,
    /// Additional source files, relative to CARGO_MANIFEST_DIR. No JSON fields are scanned.
    pub sources: Vec<PathBuf>,
}

impl EmbedRequest {
    pub fn new(name: impl Into<String>, definition: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            definition: definition.into(),
            sources: Vec::new(),
        }
    }

    /// Include a source that a declared child may reference during host preparation.
    pub fn with_source(mut self, source: impl Into<PathBuf>) -> Self {
        self.sources.push(source.into());
        self
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
    #[snafu(display("embedded source {path:?} must be inside package directory {root:?}"))]
    OutsidePackage { path: PathBuf, root: PathBuf },
    #[snafu(display("embedded source {path:?} must not traverse symbolic links"))]
    LinkedSource { path: PathBuf },
    #[snafu(display("embedded source {path:?} must be a regular UTF-8 source file"))]
    InvalidSourceFile { path: PathBuf },
    #[snafu(display("embedding output directory {path:?} must not be a symbolic link"))]
    LinkedOutput { path: PathBuf },
    #[snafu(display("could not create embedding output directory {path:?}: {source}"))]
    Directory { path: PathBuf, source: io::Error },
    #[snafu(transparent)]
    State { source: crate::StateError },
}

/// Check workflow structure and emit source modules into Cargo's OUT_DIR.
///
/// Paths are relative to CARGO_MANIFEST_DIR and must stay inside that package. The root
/// definition and explicitly listed sources are embedded without consulting node metadata.
/// Generated `plan(registry)` and preparation entry points resolve declared child fields
/// against the host registry, using only those embedded sources and host builtin Flows.
/// Providers, including mfn-core, are needed only as normal host dependencies.
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
    if requests.is_empty() {
        return Ok(());
    }
    let logical_manifest = manifest;
    let manifest = fs::canonicalize(manifest).context(ReadSnafu {
        name: "package",
        path: manifest,
    })?;
    let mut modules = Vec::new();
    for request in requests {
        let name = &request.name;
        let mut sources = BTreeMap::new();
        let entry = collect_source(
            name,
            &request.definition,
            logical_manifest,
            &manifest,
            &mut sources,
        )?;
        let path = manifest.join(&entry);
        let definition = WorkflowDefinition::from_json(&sources[&entry])
            .context(DefinitionSnafu { name, path: &path })?;
        // Validate only the authored root graph. Declared children remain opaque until
        // the host supplies its registry; an incomplete plan is never serialized.
        crate::compiler::structural_order_graph(&definition)
            .context(PlanningSnafu { name, path: &path })?;
        for source in &request.sources {
            collect_source(name, source, logical_manifest, &manifest, &mut sources)?;
        }
        let workflow = crate::FlowSourceBundle { entry, sources };
        let source = crate::plan::generate_embedded_source(&workflow)
            .context(GenerationSnafu { name, path: &path })?;
        modules.push((output.join("mf-embedded").join(name), source));
    }
    // A structural failure must preserve every module from the previous successful build.
    create_output_directory(&output.join("mf-embedded"))?;
    for (directory, _) in &modules {
        create_output_directory(directory)?;
    }
    for (directory, source) in modules {
        crate::write_if_changed(&directory.join("workflow.rs"), source.as_bytes())?;
    }
    Ok(())
}

fn collect_source(
    name: &str,
    path: &Path,
    logical_manifest: &Path,
    manifest: &Path,
    sources: &mut BTreeMap<PathBuf, String>,
) -> Result<PathBuf, EmbeddingError> {
    let relative = if path.is_absolute() {
        path.strip_prefix(logical_manifest)
            .or_else(|_| path.strip_prefix(manifest))
            .ok()
            .context(OutsidePackageSnafu {
                path,
                root: manifest,
            })?
    } else {
        path
    };
    let normalized = crate::flow_sources::normalize_embedded_path(relative)
        .context(SourcesSnafu { name, path })?;
    let mut component_path = manifest.to_path_buf();
    for component in relative.components() {
        component_path.push(component);
        let metadata = fs::symlink_metadata(&component_path).context(ReadSnafu {
            name,
            path: &component_path,
        })?;
        ensure!(
            !metadata.file_type().is_symlink(),
            LinkedSourceSnafu {
                path: &component_path
            }
        );
    }
    let relative = normalized;
    let path = manifest.join(&relative);
    println!("cargo:rerun-if-changed={}", path.display());
    let canonical = fs::canonicalize(&path).context(ReadSnafu { name, path: &path })?;
    ensure!(canonical == path, LinkedSourceSnafu { path: &path });
    let metadata = fs::symlink_metadata(&path).context(ReadSnafu { name, path: &path })?;
    ensure!(metadata.is_file(), InvalidSourceFileSnafu { path: &path });
    if !sources.contains_key(&relative) {
        let source = fs::read_to_string(&path).context(ReadSnafu { name, path: &path })?;
        sources.insert(relative.clone(), source);
    }
    Ok(relative)
}

#[cfg(test)]
mod tests {
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
    fn explicit_resources_are_packed_without_resolving_provider_metadata() {
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
        embed_workflows_at(
            &[request().with_source("flows/child.json")],
            root.path(),
            &output,
        )
        .unwrap();
        let module = fs::read_to_string(output.join("mf-embedded/answer/workflow.rs")).unwrap();
        assert!(module.contains("workflow.call"));
        assert!(module.contains("unlinked.echo"));
        assert!(module.contains("flows/child.json"));
        assert!(module.contains("workflow.resolve(registry)"));
        // Packaging never follows source-shaped configuration or named sources.
        fs::remove_file(root.path().join("flows/child.json")).unwrap();
        embed_workflows_at(&[request()], root.path(), &output).unwrap();
    }

    fn write_definition(root: &Path, value: &serde_json::Value) {
        fs::write(root.join("flow.json"), value.to_string()).unwrap();
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

    #[test]
    fn packages_only_explicit_resources_without_parsing_their_contents() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("out");
        let mut value = definition();
        value["nodes"][0]["config"] = json!({"ordinary": {"path": "missing.json"}});
        write_definition(root.path(), &value);
        fs::write(root.path().join("unused.flow"), "not JSON").unwrap();
        fs::write(root.path().join("private.txt"), "must not be embedded").unwrap();
        embed_workflows_at(
            &[request()
                .with_source("unused.flow")
                .with_source("./unused.flow")],
            root.path(),
            &output,
        )
        .unwrap();
        let module = fs::read_to_string(output.join("mf-embedded/answer/workflow.rs")).unwrap();
        assert!(module.contains("not JSON"));
        assert!(!module.contains("must not be embedded"));
        let error = embed_workflows_at(
            &[request().with_source("missing.json")],
            root.path(),
            &output,
        )
        .unwrap_err();
        assert!(matches!(error, EmbeddingError::Read { source, .. }
            if source.kind() == io::ErrorKind::NotFound));
    }

    #[test]
    fn resource_paths_are_confined_to_the_package() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write_definition(root.path(), &definition());
        fs::write(outside.path().join("secret.flow"), "not embedded").unwrap();
        for path in [
            PathBuf::from("../secret.flow"),
            outside.path().join("secret.flow"),
        ] {
            let error = embed_workflows_at(
                &[request().with_source(path)],
                root.path(),
                &root.path().join("out"),
            )
            .unwrap_err();
            assert!(
                matches!(
                    error,
                    EmbeddingError::Sources { .. } | EmbeddingError::OutsidePackage { .. }
                ),
                "{error}"
            );
        }
        // Absolute entry paths inside the package remain supported.
        let entry = fs::canonicalize(root.path().join("flow.json")).unwrap();
        embed_workflows_at(
            &[EmbedRequest::new("answer", entry)],
            root.path(),
            &root.path().join("out"),
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn absolute_sources_accept_the_logical_package_directory() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("package");
        fs::create_dir(&package).unwrap();
        write_definition(&package, &definition());
        fs::write(package.join("child.flow"), "{}").unwrap();
        let alias = root.path().join("logical-package");
        symlink(&package, &alias).unwrap();
        embed_workflows_at(
            &[EmbedRequest::new("answer", alias.join("flow.json"))
                .with_source(alias.join("child.flow"))],
            &alias,
            &root.path().join("out"),
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn special_files_are_rejected_before_reading() {
        let root = tempfile::tempdir().unwrap();
        write_definition(root.path(), &definition());
        let pipe = root.path().join("pipe.flow");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&pipe)
                .status()
                .unwrap()
                .success()
        );
        fs::create_dir(root.path().join("directory.flow")).unwrap();
        for source in [Path::new("pipe.flow"), Path::new("directory.flow")] {
            let error = embed_workflows_at(
                &[request().with_source(source)],
                root.path(),
                &root.path().join("out"),
            )
            .unwrap_err();
            assert!(
                matches!(error, EmbeddingError::InvalidSourceFile { .. }),
                "{error}"
            );
        }
    }

    #[test]
    fn source_backed_loop_is_not_interpreted_during_packaging() {
        let root = tempfile::tempdir().unwrap();
        let value = json!({
            "version": "2026-10-28", "dependencies": {}, "nodes": [{
                "id": "repeat", "kind": "workflow.loop", "config": {
                    "max_iterations": 1, "variables": [],
                    "body": {"path": "not-included.flow"}
                }
            }]
        });
        write_definition(root.path(), &value);
        embed_workflows_at(&[request()], root.path(), &root.path().join("out")).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_source_links_instead_of_changing_virtual_path_semantics() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        write_definition(root.path(), &definition());
        symlink("flow.json", root.path().join("alias.flow")).unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        fs::write(root.path().join("nested/child.flow"), "{}").unwrap();
        symlink("nested", root.path().join("alias-dir")).unwrap();
        for source in [
            "alias.flow",
            "alias-dir/child.flow",
            "alias-dir/../flow.json",
        ] {
            let error = embed_workflows_at(
                &[request().with_source(source)],
                root.path(),
                &root.path().join("out"),
            )
            .unwrap_err();
            assert!(
                matches!(error, EmbeddingError::LinkedSource { .. }),
                "{error}"
            );
        }
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
