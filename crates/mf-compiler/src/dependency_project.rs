use crate::{GeneratedWorkflowArtifacts, WorkflowDefinition};
use snafu::{ResultExt, Snafu};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub enum SupportPackages {
    Registry,
    Local { crates_dir: PathBuf },
}

#[derive(Debug, Snafu)]
pub enum DependencyProjectError {
    #[snafu(display("invalid dependency {alias}: {message}"))]
    Dependency { alias: String, message: String },
    #[snafu(display("could not serialize runner input: {source}"))]
    Serialize { source: serde_json::Error },
    #[snafu(display("could not write generated project at {path:?}: {source}"))]
    Write { path: PathBuf, source: io::Error },
}

const MAIN: &str = r#"mod workflow;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => { eprintln!("{error}"); std::process::ExitCode::FAILURE }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let registry = mf_runtime::NodeRegistry::from_inventory()?;
    if args.len() == 1 && args[0] == "--validate" {
        let plan = mf_compiler::CompiledWorkflow::from_json(include_str!("../workflow-plan.json"))?;
        mf_compiler::instantiate_compiled(&plan, &registry)?;
        return Ok(());
    }
    if !args.is_empty() {
        return Err("usage: workflow [--validate]".into());
    }
    let outputs = workflow::run_workflow(&registry)?;
    println!("{}", serde_json::to_string(&outputs)?);
    Ok(())
}
"#;

fn quoted(value: &str) -> Result<String, DependencyProjectError> {
    serde_json::to_string(value).context(SerializeSnafu)
}

fn path_string(path: &Path) -> Result<String, DependencyProjectError> {
    let path = fs::canonicalize(path).context(WriteSnafu {
        path: path.to_owned(),
    })?;
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| DependencyProjectError::Dependency {
            alias: "path".into(),
            message: "path is not UTF-8".into(),
        })
}

pub fn dependency_project_files(
    definition: &WorkflowDefinition,
    artifacts: &GeneratedWorkflowArtifacts,
    support: &SupportPackages,
) -> Result<BTreeMap<PathBuf, String>, DependencyProjectError> {
    let mut manifest = String::from(
        "[package]\nname = \"mf-generated-workflow\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\nserde_json = \"1.0.151\"\n",
    );
    for package in ["mf-runtime", "mf-compiler"] {
        let source = match support {
            SupportPackages::Registry => format!(
                "version = {}",
                quoted(&format!("={}", env!("CARGO_PKG_VERSION")))?
            ),
            SupportPackages::Local { crates_dir } => format!(
                "path = {}",
                quoted(&path_string(&crates_dir.join(package))?)?
            ),
        };
        manifest.push_str(&format!("{package} = {{ {source} }}\n"));
    }
    let mut main = String::new();
    for (index, (alias, dependency)) in definition.dependencies.iter().enumerate() {
        dependency
            .validate()
            .map_err(|message| DependencyProjectError::Dependency {
                alias: alias.clone(),
                message,
            })?;
        let name = format!("node_{index}");
        main.push_str(&format!("extern crate {name} as _;\n"));
        let mut fields = vec![format!("package = {}", quoted(&dependency.package)?)];
        if let Some(version) = &dependency.version {
            fields.push(format!("version = {}", quoted(version)?));
        }
        if let Some(git) = &dependency.git {
            fields.push(format!("git = {}", quoted(git)?));
        }
        if let Some(rev) = &dependency.rev {
            fields.push(format!("rev = {}", quoted(rev)?));
        }
        if let Some(path) = &dependency.path {
            fields.push(format!("path = {}", quoted(&path_string(path)?)?));
        }
        fields.push(format!(
            "default-features = {}",
            dependency.default_features
        ));
        let features: Vec<_> = dependency
            .features
            .iter()
            .map(|f| quoted(f))
            .collect::<Result<_, _>>()?;
        fields.push(format!("features = [{}]", features.join(", ")));
        manifest.push_str(&format!("{name} = {{ {} }}\n", fields.join(", ")));
    }
    main.push_str(MAIN);
    let mut files = BTreeMap::from([
        (PathBuf::from("Cargo.toml"), manifest),
        (PathBuf::from("src/main.rs"), main),
        (
            PathBuf::from("src/workflow.rs"),
            artifacts.rust_source.clone(),
        ),
        (
            PathBuf::from("workflow-plan.json"),
            artifacts.plan_json.clone(),
        ),
    ]);
    for (name, value) in &artifacts.config_files {
        if Path::new(name).file_name().and_then(|n| n.to_str()) != Some(name)
            || !name.starts_with("config_")
            || !name.ends_with(".json")
        {
            return Err(DependencyProjectError::Dependency {
                alias: name.clone(),
                message: "invalid generated configuration file name".into(),
            });
        }
        files.insert(PathBuf::from("src").join(name), value.clone());
    }
    Ok(files)
}

pub fn write_dependency_project(
    project: &Path,
    files: &BTreeMap<PathBuf, String>,
) -> Result<(), DependencyProjectError> {
    fs::create_dir_all(project.join("src")).context(WriteSnafu {
        path: project.to_owned(),
    })?;
    for (name, value) in files {
        let path = project.join(name);
        fs::write(&path, value).context(WriteSnafu { path })?;
    }
    Ok(())
}
