use serde_json::Value;
use snafu::{ResultExt, Snafu};
use std::{
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

#[derive(Debug, Snafu)]
pub enum CargoBuildError {
    #[snafu(display("{stage} failed at {project:?}: {source}"))]
    Io {
        stage: &'static str,
        project: PathBuf,
        source: io::Error,
    },
    #[snafu(display("{stage} failed with {status}; generated project kept at {project:?}"))]
    Process {
        stage: &'static str,
        project: PathBuf,
        status: ExitStatus,
    },
    #[snafu(display("invalid Cargo metadata at {project:?}: {source}"))]
    Metadata {
        project: PathBuf,
        source: serde_json::Error,
    },
    #[snafu(display("--locked requires an existing dependency lock at {path:?}"))]
    MissingLock { path: PathBuf },
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
) -> Result<Value, CargoBuildError> {
    let working = project.join("Cargo.lock");
    match fs::read(flow_lock) {
        Ok(contents) => {
            fs::write(&working, contents).context(IoSnafu {
                stage: "lock synchronization",
                project: project.to_owned(),
            })?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if locked {
                return MissingLockSnafu {
                    path: flow_lock.to_owned(),
                }
                .fail();
            }
            if let Err(source) = fs::remove_file(&working)
                && source.kind() != io::ErrorKind::NotFound
            {
                return Err(CargoBuildError::Io {
                    stage: "lock synchronization",
                    project: project.to_owned(),
                    source,
                });
            }
        }
        Err(source) => {
            return Err(CargoBuildError::Io {
                stage: "lock synchronization",
                project: project.to_owned(),
                source,
            });
        }
    }
    let mut command = cargo_command(project);
    command.args(["metadata", "--format-version", "1"]);
    if locked {
        command.arg("--locked");
    }
    let output = command.output().context(IoSnafu {
        stage: "dependency resolution",
        project: project.to_owned(),
    })?;
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return ProcessSnafu {
            stage: "dependency resolution",
            project: project.to_owned(),
            status: output.status,
        }
        .fail();
    }
    serde_json::from_slice(&output.stdout).context(MetadataSnafu {
        project: project.to_owned(),
    })
}
