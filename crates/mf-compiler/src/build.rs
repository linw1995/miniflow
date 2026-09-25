use crate::{GeneratedWorkflowArtifacts, RunnerProjectError, write_runner_project};
use snafu::{ResultExt, Snafu};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum BinaryBuildError {
    #[snafu(display("output path {path:?} has no file name"))]
    InvalidOutputPath { path: PathBuf },
    #[snafu(display("could not access output directory {path:?}: {source}"))]
    OutputDirectory { path: PathBuf, source: io::Error },
    #[snafu(display("could not create temporary build directory {path:?}: {source}"))]
    CreateBuildDirectory { path: PathBuf, source: io::Error },
    #[snafu(display("could not generate runner project: {source}"))]
    GenerateProject { source: RunnerProjectError },
    #[snafu(display("could not start Cargo; generated project kept at {project:?}: {source}"))]
    StartCargo { project: PathBuf, source: io::Error },
    #[snafu(display("Cargo build failed with {status}; generated project kept at {project:?}"))]
    CargoBuildFailed {
        status: ExitStatus,
        project: PathBuf,
    },
    #[snafu(display("could not stage executable from {source_path:?} to {target:?}: {source}"))]
    StageExecutable {
        source_path: PathBuf,
        target: PathBuf,
        source: io::Error,
    },
    #[snafu(display(
        "could not install executable at {target:?}; generated project kept at {project:?}: {source}"
    ))]
    InstallExecutable {
        target: PathBuf,
        project: PathBuf,
        source: io::Error,
    },
}

pub fn build_executable(
    artifacts: &GeneratedWorkflowArtifacts,
    output: &Path,
    runtime_crate: &Path,
    bundle_crate: &Path,
) -> Result<(), BinaryBuildError> {
    let Some(file_name) = output.file_name() else {
        return InvalidOutputPathSnafu {
            path: output.to_path_buf(),
        }
        .fail();
    };
    let parent = output.parent().filter(|path| !path.as_os_str().is_empty());
    let parent = parent.unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent).context(OutputDirectorySnafu {
        path: parent.to_path_buf(),
    })?;
    let target = parent.join(file_name);
    let mut build_dir = BuildDirectory::new(&parent)?;
    let project = build_dir.path().join("project");
    write_runner_project(artifacts, &project, runtime_crate, bundle_crate)
        .context(GenerateProjectSnafu)?;

    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let status = Command::new(cargo)
        .arg("build")
        .arg("--release")
        .arg("--manifest-path")
        .arg(project.join("Cargo.toml"))
        .current_dir(&project)
        .env("CARGO_TARGET_DIR", project.join("target"))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    let status = match status {
        Ok(status) => status,
        Err(source) => {
            build_dir.preserve();
            return Err(BinaryBuildError::StartCargo { project, source });
        }
    };
    if !status.success() {
        build_dir.preserve();
        return CargoBuildFailedSnafu { status, project }.fail();
    }

    let executable_name = if cfg!(windows) {
        "mf-generated-workflow.exe"
    } else {
        "mf-generated-workflow"
    };
    let source_path = project.join("target/release").join(executable_name);
    let staged = build_dir.path().join("executable");
    if let Err(source) = fs::copy(&source_path, &staged) {
        build_dir.preserve();
        return Err(BinaryBuildError::StageExecutable {
            source_path,
            target: staged,
            source,
        });
    }
    if let Err(source) = fs::rename(&staged, &target) {
        build_dir.preserve();
        return Err(BinaryBuildError::InstallExecutable {
            target,
            project,
            source,
        });
    }
    Ok(())
}

struct BuildDirectory {
    path: PathBuf,
    preserve: bool,
}

impl BuildDirectory {
    fn new(parent: &Path) -> Result<Self, BinaryBuildError> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for attempt in 0..32 {
            let path = parent.join(format!(
                ".mf-build-{}-{nonce}-{attempt}",
                std::process::id()
            ));
            match create_private_directory(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        preserve: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(BinaryBuildError::CreateBuildDirectory { path, source }),
            }
        }
        let path = parent.join(format!(".mf-build-{}-{nonce}", std::process::id()));
        Err(BinaryBuildError::CreateBuildDirectory {
            path,
            source: io::Error::new(io::ErrorKind::AlreadyExists, "no unique build directory"),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn preserve(&mut self) {
        self.preserve = true;
    }
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path)
    }
}

impl Drop for BuildDirectory {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
