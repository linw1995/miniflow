use crate::{
    BuildGuard, BuildInputs, SupportPackages, WorkflowDefinition, atomic_copy, plan_definition,
    validate_runtime_identity, write_dependency_project,
};
use snafu::Snafu;
use std::{
    env,
    error::Error,
    ffi::OsString,
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
    let guard_path = inputs.lock.with_extension("lock.guard");
    if output == guard_path {
        return Err(failure(
            "output validation",
            &inputs.definition,
            "output would replace the dependency build guard",
        ));
    }
    let _guard = at(
        "dependency lock",
        &inputs.definition,
        BuildGuard::acquire(&guard_path),
    )?;
    let directory = at(
        "build directory",
        &inputs.definition,
        crate::BuildDirectory::open_protected(
            &inputs.definition,
            request.build_dir,
            &[&inputs.definition, &inputs.lock, &output],
        ),
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
    at(
        "project generation",
        &project,
        write_dependency_project(&project, &plan, request.support),
    )?;
    let metadata = resolve_project(&project, &inputs.lock, request.locked)?;
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
            crate::state::write_if_changed(&inputs.lock, &lock),
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

fn build_runner(project: &Path) -> Result<PathBuf, PipelineError> {
    let mut child = at(
        "Cargo build",
        project,
        cargo_command(project)
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
                if value["fresh"] == true {
                    eprintln!("reused compiled runner");
                }
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

fn validate_runner(project: &Path, executable: &Path) -> Result<(), PipelineError> {
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

pub fn cargo_command(project: &Path) -> Command {
    let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
    command
        .current_dir(project)
        .env("CARGO_TARGET_DIR", project.join("target"));
    command
}

pub fn resolve_project(
    project: &Path,
    flow_lock: &Path,
    locked: bool,
) -> Result<serde_json::Value, PipelineError> {
    let working = project.join("Cargo.lock");
    match fs::read(flow_lock) {
        Ok(contents) => at(
            "lock synchronization",
            project,
            crate::state::write_if_changed(&working, &contents),
        )?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if locked {
                return Err(failure(
                    "dependency resolution",
                    project,
                    format!("--locked requires an existing dependency lock at {flow_lock:?}"),
                ));
            }
            if let Err(error) = fs::remove_file(&working)
                && error.kind() != io::ErrorKind::NotFound
            {
                return at("lock synchronization", project, Err(error));
            }
        }
        Err(error) => return at("lock synchronization", project, Err(error)),
    }
    let mut command = cargo_command(project);
    command.args(["metadata", "--format-version", "1"]);
    if locked {
        command.arg("--locked");
    }
    let output = at("dependency resolution", project, command.output())?;
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(failure(
            "dependency resolution",
            project,
            format!("Cargo metadata exited with {}", output.status),
        ));
    }
    at(
        "dependency resolution",
        project,
        serde_json::from_slice(&output.stdout),
    )
}
