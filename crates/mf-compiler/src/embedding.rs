use crate::{NodeRegistry, WorkflowDefinition};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::BTreeSet,
    env, fs, io,
    path::{Path, PathBuf},
};

/// A workflow and its provider aliases in the host's Cargo manifest.
#[derive(Clone, Debug)]
pub struct EmbedRequest {
    pub name: String,
    pub definition: PathBuf,
    providers: Vec<(String, String)>,
}

impl EmbedRequest {
    pub fn new(name: impl Into<String>, definition: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            definition: definition.into(),
            providers: Vec::new(),
        }
    }

    /// Bind a workflow dependency alias to a Rust crate name in the host.
    pub fn with_provider(
        mut self,
        alias: impl Into<String>,
        crate_name: impl Into<String>,
    ) -> Self {
        self.providers.push((alias.into(), crate_name.into()));
        self
    }
}

#[derive(Debug, Snafu)]
pub enum EmbeddingError {
    #[snafu(display("workflow embedding requires Cargo environment variable {variable}"))]
    CargoEnvironment { variable: &'static str },
    #[snafu(display("embedded workflow name `{name}` must be a portable Rust module identifier"))]
    InvalidName { name: String },
    #[snafu(display("embedded workflow name `{name}` occurs more than once in the request batch"))]
    DuplicateName { name: String },
    #[snafu(display("embedded workflow `{name}` provider `{alias}`: {message}"))]
    ProviderBinding {
        name: String,
        alias: String,
        message: &'static str,
    },
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

/// Validate workflows with linked build providers and emit modules into Cargo's OUT_DIR.
///
/// Paths are relative to CARGO_MANIFEST_DIR. The host owns dependency resolution and must link
/// compatible provider crates in both its build script and target. Submit names in one batch.
pub fn embed_workflows(
    requests: &[EmbedRequest],
    registry: &NodeRegistry,
) -> Result<(), EmbeddingError> {
    let manifest = env::var_os("CARGO_MANIFEST_DIR").context(CargoEnvironmentSnafu {
        variable: "CARGO_MANIFEST_DIR",
    })?;
    let output = env::var_os("OUT_DIR").context(CargoEnvironmentSnafu {
        variable: "OUT_DIR",
    })?;
    embed_workflows_at(requests, registry, Path::new(&manifest), Path::new(&output))
}

fn portable_identifier(name: &str) -> bool {
    name != "_"
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && syn::parse_str::<syn::Ident>(name).is_ok()
}

fn validate_providers(
    request: &EmbedRequest,
    definition: &WorkflowDefinition,
) -> Result<(), EmbeddingError> {
    let mut aliases = BTreeSet::new();
    for (alias, crate_name) in &request.providers {
        ensure!(
            portable_identifier(crate_name),
            ProviderBindingSnafu {
                name: &request.name,
                alias,
                message: "host crate name must be a portable Rust identifier",
            }
        );
        ensure!(
            definition.dependencies.contains_key(alias),
            ProviderBindingSnafu {
                name: &request.name,
                alias,
                message: "binding references an undeclared workflow dependency",
            }
        );
        ensure!(
            aliases.insert(alias),
            ProviderBindingSnafu {
                name: &request.name,
                alias,
                message: "binding occurs more than once",
            }
        );
    }
    for alias in definition.dependencies.keys() {
        ensure!(
            aliases.contains(alias),
            ProviderBindingSnafu {
                name: &request.name,
                alias,
                message: "workflow dependency has no host crate binding",
            }
        );
    }
    Ok(())
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
    registry: &NodeRegistry,
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
    let mut batches = Vec::new();
    for request in requests {
        let name = &request.name;
        let path = manifest.join(&request.definition);
        println!("cargo:rerun-if-changed={}", path.display());
        let source = fs::read_to_string(&path).context(ReadSnafu { name, path: &path })?;
        let definition = WorkflowDefinition::from_json(&source)
            .context(DefinitionSnafu { name, path: &path })?;
        validate_providers(request, &definition)?;
        let plan =
            crate::plan_definition(&definition).context(PlanningSnafu { name, path: &path })?;
        let layouts = plan
            .generate_execution_plans(registry)
            .context(GenerationSnafu { name, path: &path })?;
        let directory = format!("mf-embedded/{name}");
        let module = plan
            .generate_embedded_artifacts(&directory)
            .context(GenerationSnafu { name, path: &path })?;
        let providers: BTreeSet<_> = request
            .providers
            .iter()
            .map(|(_, crate_name)| crate_name)
            .collect();
        let providers = providers
            .into_iter()
            .map(|name| quote::format_ident!("{name}"));
        let anchors = quote::quote! { #(extern crate #providers as _;)* };
        let module_source = format!("{anchors}\n{}", module.rust_source);
        batches.push((
            output.join(directory),
            [
                ("flow-plans.rs", layouts.rust_source.into_bytes()),
                ("workflow-manifest.bin", layouts.manifest_bytes),
                ("workflow.rs", module_source.into_bytes()),
            ],
        ));
    }
    // Validate the complete batch before publishing any module.
    if batches.is_empty() {
        return Ok(());
    }
    create_output_directory(&output.join("mf-embedded"))?;
    for (directory, _) in &batches {
        create_output_directory(directory)?;
    }
    for (directory, files) in batches {
        for (file, contents) in files {
            crate::write_if_changed(&directory.join(file), &contents)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mfn_core as _;
    use serde_json::json;

    fn definition() -> serde_json::Value {
        json!({
            "version": "2026-10-03",
            "dependencies": {"core": {"package": "mfn-core", "version": "0.1.0"}},
            "nodes": [{"id": "source", "kind": "builtin.constant", "config": {"value": 42}}],
            "outputs": [{"name": "answer", "node": "source", "port": "value"}]
        })
    }

    fn request() -> EmbedRequest {
        EmbedRequest::new("answer", "flow.json").with_provider("core", "core_nodes")
    }

    fn write_definition(root: &Path, value: &serde_json::Value) {
        fs::write(root.join("flow.json"), value.to_string()).unwrap();
    }

    #[test]
    fn rejects_invalid_requests_before_replacing_existing_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let registry = NodeRegistry::from_inventory().unwrap();
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
                &[EmbedRequest {
                    name: name.into(),
                    ..request()
                }],
                &registry,
                root.path(),
                &output,
            )
            .unwrap_err();
            assert!(
                matches!(error, EmbeddingError::InvalidName { .. }),
                "{error}"
            );
        }
        let error = embed_workflows_at(&[request(), request()], &registry, root.path(), &output)
            .unwrap_err();
        assert!(matches!(error, EmbeddingError::DuplicateName { .. }));
        for invalid in [
            EmbedRequest::new("answer", "flow.json"),
            request().with_provider("core", "core_nodes"),
            request().with_provider("extra", "core_nodes"),
            EmbedRequest::new("answer", "flow.json").with_provider("core", "core-nodes"),
        ] {
            let error =
                embed_workflows_at(&[invalid], &registry, root.path(), &output).unwrap_err();
            assert!(
                matches!(error, EmbeddingError::ProviderBinding { .. }),
                "{error}"
            );
        }
        let mut invalid = definition();
        fs::write(root.path().join("valid.json"), invalid.to_string()).unwrap();
        invalid["nodes"][0]["kind"] = json!("unknown.kind");
        write_definition(root.path(), &invalid);
        let error = embed_workflows_at(
            &[
                EmbedRequest {
                    definition: "valid.json".into(),
                    ..request()
                },
                EmbedRequest {
                    name: "other".into(),
                    ..request()
                },
            ],
            &registry,
            root.path(),
            &output,
        )
        .unwrap_err();
        assert!(matches!(error, EmbeddingError::Generation { .. }));
        assert_eq!(fs::read_to_string(file).unwrap(), "sentinel");
    }

    #[test]
    fn preserves_definition_and_validation_error_sources() {
        use std::error::Error;
        let root = tempfile::tempdir().unwrap();
        let registry = NodeRegistry::from_inventory().unwrap();
        let output = root.path().join("out");
        fs::write(root.path().join("flow.json"), "{").unwrap();
        let error = embed_workflows_at(&[request()], &registry, root.path(), &output).unwrap_err();
        assert!(error.source().unwrap().is::<crate::DefinitionParseError>());
        let mut value = definition();
        value["nodes"][0]["config"] = json!({});
        write_definition(root.path(), &value);
        let error = embed_workflows_at(&[request()], &registry, root.path(), &output).unwrap_err();
        let mut source = error.source();
        let mut found = false;
        while let Some(error) = source {
            found |= error.is::<crate::NodeBuildError>();
            source = error.source();
        }
        assert!(found, "{error}");
        assert!(!output.exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_output_directory_links() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let registry = NodeRegistry::from_inventory().unwrap();
        write_definition(root.path(), &definition());
        let outside = root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        for relative in ["mf-embedded", "mf-embedded/answer"] {
            let output = tempfile::tempdir().unwrap();
            let link = output.path().join(relative);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(&outside, &link).unwrap();
            let error = embed_workflows_at(&[request()], &registry, root.path(), output.path())
                .unwrap_err();
            assert!(matches!(error, EmbeddingError::LinkedOutput { .. }));
        }
        assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
    }
}
