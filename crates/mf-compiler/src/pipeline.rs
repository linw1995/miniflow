use crate::{
    BuildGuard, BuildInputs, SupportPackages, WorkflowDefinition, atomic_copy, atomic_write,
    dependency_project_files, plan_definition, resolve_project, validate_runtime_identity,
    write_dependency_project,
};
use snafu::Snafu;
use std::{
    error::Error,
    fs,
    io::{self, BufRead},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Debug, Snafu)]
#[snafu(display("{stage} failed; build project at {project:?}: {source}"))]
pub struct PipelineError {
    pub stage: &'static str,
    pub project: PathBuf,
    pub source: Box<dyn Error + Send + Sync>,
}

fn at<T, E: Error + Send + Sync + 'static>(
    stage: &'static str,
    project: &Path,
    result: Result<T, E>,
) -> Result<T, PipelineError> {
    result.map_err(|source| PipelineError {
        stage,
        project: project.to_owned(),
        source: Box::new(source),
    })
}
fn failure(stage: &'static str, project: &Path, message: impl Into<String>) -> PipelineError {
    PipelineError {
        stage,
        project: project.to_owned(),
        source: Box::new(io::Error::other(message.into())),
    }
}

pub struct CompileRequest<'a> {
    pub definition: &'a Path,
    pub output: &'a Path,
    pub locked: bool,
    pub build_dir: Option<&'a Path>,
    pub support: &'a SupportPackages,
}

pub fn compile_project(request: &CompileRequest<'_>) -> Result<PathBuf, PipelineError> {
    let inputs = at(
        "input resolution",
        request.definition,
        BuildInputs::new(request.definition),
    )?;
    let output = at(
        "output validation",
        request.definition,
        inputs.check_output(request.output),
    )?;
    let source = at(
        "definition read",
        &inputs.definition,
        fs::read_to_string(&inputs.definition),
    )?;
    let definition = at(
        "definition parsing",
        &inputs.definition,
        WorkflowDefinition::from_json(&source),
    )?;
    let definition = at(
        "dependency paths",
        &inputs.definition,
        inputs.resolve_dependencies(&definition),
    )?;
    let plan = at(
        "structural validation",
        &inputs.definition,
        plan_definition(&definition),
    )?;
    let artifacts = at(
        "code generation",
        &inputs.definition,
        plan.generate_artifacts(),
    )?;
    let guard_path = inputs.lock.with_extension("lock.guard");
    let _guard = at(
        "dependency lock",
        &inputs.definition,
        BuildGuard::acquire(&guard_path),
    )?;
    let directory = at(
        "build directory",
        &inputs.definition,
        crate::BuildDirectory::open(&inputs.definition, request.build_dir),
    )?;
    let project = directory.path.clone();
    eprintln!(
        "{} build directory {project:?}",
        if directory.reused {
            "reused"
        } else {
            "created"
        }
    );
    let files = at(
        "project generation",
        &project,
        dependency_project_files(&definition, &artifacts, request.support),
    )?;
    at(
        "project generation",
        &project,
        write_dependency_project(&project, &files),
    )?;
    let metadata = at(
        "dependency resolution",
        &project,
        resolve_project(&project, &inputs.lock, request.locked),
    )?;
    at(
        "runtime compatibility",
        &project,
        validate_runtime_identity(&metadata),
    )?;
    let executable = build_runner(&project)?;
    validate_runner(&project, &executable)?;
    if !request.locked {
        let lock = at("lock read", &project, fs::read(project.join("Cargo.lock")))?;
        at(
            "lock persistence",
            &project,
            atomic_write(&inputs.lock, &lock),
        )?;
    }
    let stage = if request.locked {
        "executable installation"
    } else {
        "executable installation (dependency lock was updated)"
    };
    at(stage, &project, atomic_copy(&executable, &output))?;
    Ok(project)
}

pub fn build_runner(project: &Path) -> Result<PathBuf, PipelineError> {
    let mut child = at(
        "Cargo build",
        project,
        crate::cargo_build::cargo_command(project)
            .args([
                "build",
                "--release",
                "--locked",
                "--message-format=json-render-diagnostics",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn(),
    )?;
    let mut executable = None;
    let stream = child.stdout.take().unwrap();
    let mut stream_error = None;
    for line in io::BufReader::new(stream).lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                stream_error = Some(error);
                break;
            }
        };
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
            if value["reason"] == "compiler-artifact"
                && value["target"]["name"] == "mf-generated-workflow"
            {
                executable = value["executable"].as_str().map(PathBuf::from);
            }
            if let Some(rendered) = value["message"]["rendered"].as_str() {
                eprint!("{rendered}");
            }
        } else {
            eprintln!("{line}");
        }
    }
    let status = at("Cargo build", project, child.wait())?;
    if let Some(error) = stream_error {
        return Err(failure("Cargo diagnostics", project, error.to_string()));
    }
    if !status.success() {
        return Err(failure(
            "Cargo build",
            project,
            format!("Cargo build failed with {status}"),
        ));
    }
    executable.filter(|path| path.is_file()).ok_or_else(|| {
        failure(
            "Cargo build",
            project,
            "Cargo did not produce a runner executable",
        )
    })
}

pub fn validate_runner(project: &Path, executable: &Path) -> Result<(), PipelineError> {
    let status = at(
        "runner validation",
        project,
        Command::new(executable)
            .arg("--validate")
            .current_dir(project)
            .stdin(Stdio::null())
            .status(),
    )?;
    if !status.success() {
        return Err(failure(
            "runner validation",
            project,
            format!("validation exited with {status}"),
        ));
    }
    Ok(())
}
