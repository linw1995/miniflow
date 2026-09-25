use crate::GeneratedWorkflowArtifacts;
use snafu::{ResultExt, Snafu};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

const RUNNER_MAIN: &str = r#"mod workflow;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = mf_bundle::registry()?;
    let outputs = workflow::run_workflow(&registry)?;
    println!("{}", serde_json::to_string(&outputs)?);
    Ok(())
}
"#;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum RunnerProjectError {
    #[snafu(display("could not resolve dependency path {path:?}: {source}"))]
    ResolveDependency { path: PathBuf, source: io::Error },
    #[snafu(display("dependency path {path:?} is not valid UTF-8"))]
    NonUtf8DependencyPath { path: PathBuf },
    #[snafu(display("could not serialize dependency path {path:?}: {source}"))]
    SerializeDependencyPath {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[snafu(display("invalid generated configuration file name `{name}`"))]
    InvalidConfigFileName { name: String },
    #[snafu(display("could not create runner directory {path:?}: {source}"))]
    CreateDirectory { path: PathBuf, source: io::Error },
    #[snafu(display("could not write runner file {path:?}: {source}"))]
    WriteFile { path: PathBuf, source: io::Error },
}

pub fn write_runner_project(
    artifacts: &GeneratedWorkflowArtifacts,
    project_dir: &Path,
    runtime_crate: &Path,
    bundle_crate: &Path,
) -> Result<(), RunnerProjectError> {
    let runtime_path = dependency_path(runtime_crate)?;
    let bundle_path = dependency_path(bundle_crate)?;
    for name in artifacts.config_files.keys() {
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
            || !name.ends_with(".json")
        {
            return InvalidConfigFileNameSnafu { name: name.clone() }.fail();
        }
    }

    let manifest = format!(
        "[package]\nname = \"mf-generated-workflow\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\nmf-runtime = {{ path = {runtime_path} }}\nmf-bundle = {{ path = {bundle_path} }}\nserde_json = \"1.0.151\"\n"
    );
    create_directory(project_dir)?;
    let source_dir = project_dir.join("src");
    create_directory(&source_dir)?;
    write_file(&project_dir.join("Cargo.toml"), &manifest)?;
    write_file(
        &project_dir.join("workflow-plan.json"),
        &artifacts.plan_json,
    )?;
    write_file(&source_dir.join("main.rs"), RUNNER_MAIN)?;
    write_file(&source_dir.join("workflow.rs"), &artifacts.rust_source)?;
    for (name, config) in &artifacts.config_files {
        write_file(&source_dir.join(name), config)?;
    }
    Ok(())
}

fn dependency_path(path: &Path) -> Result<String, RunnerProjectError> {
    let canonical = fs::canonicalize(path).context(ResolveDependencySnafu {
        path: path.to_path_buf(),
    })?;
    let value = canonical
        .to_str()
        .ok_or_else(|| RunnerProjectError::NonUtf8DependencyPath {
            path: canonical.clone(),
        })?;
    serde_json::to_string(value).context(SerializeDependencyPathSnafu { path: canonical })
}

fn create_directory(path: &Path) -> Result<(), RunnerProjectError> {
    fs::create_dir(path).context(CreateDirectorySnafu {
        path: path.to_path_buf(),
    })
}

fn write_file(path: &Path, contents: &str) -> Result<(), RunnerProjectError> {
    fs::write(path, contents).context(WriteFileSnafu {
        path: path.to_path_buf(),
    })
}
