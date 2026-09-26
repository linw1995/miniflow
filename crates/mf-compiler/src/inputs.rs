use crate::WorkflowDefinition;
use snafu::{ResultExt, Snafu};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Snafu)]
pub enum InputError {
    #[snafu(display(
        "output or lock path {path:?} would overwrite the source definition or dependency lock"
    ))]
    Collision { path: PathBuf },
    #[snafu(display("could not resolve build input {path:?}: {source}"))]
    Resolve { path: PathBuf, source: io::Error },
}

#[derive(Debug)]
pub struct BuildInputs {
    pub definition: PathBuf,
    pub lock: PathBuf,
}

impl BuildInputs {
    pub fn new(definition: &Path) -> Result<Self, InputError> {
        let definition = fs::canonicalize(definition).context(ResolveSnafu {
            path: definition.to_owned(),
        })?;
        let lock = definition.with_extension("lock");
        let lock = canonical_destination(&lock)?;
        if lock == definition {
            return CollisionSnafu { path: lock }.fail();
        }
        Ok(Self { definition, lock })
    }

    pub fn check_output(&self, output: &Path) -> Result<PathBuf, InputError> {
        let output = canonical_destination(output)?;
        if output == self.definition || output == self.lock {
            return CollisionSnafu { path: output }.fail();
        }
        Ok(output)
    }

    pub fn resolve_dependencies(
        &self,
        definition: &WorkflowDefinition,
    ) -> Result<WorkflowDefinition, InputError> {
        let mut resolved = definition.clone();
        for dependency in resolved.dependencies.values_mut() {
            if let Some(path) = &mut dependency.path {
                let absolute = self.definition.parent().unwrap().join(&*path);
                *path = fs::canonicalize(&absolute).context(ResolveSnafu { path: absolute })?;
            }
        }
        Ok(resolved)
    }
}

fn canonical_destination(path: &Path) -> Result<PathBuf, InputError> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let parent = fs::canonicalize(parent).context(ResolveSnafu {
                path: parent.to_owned(),
            })?;
            let name = path.file_name().ok_or_else(|| InputError::Collision {
                path: path.to_owned(),
            })?;
            Ok(parent.join(name))
        }
        Err(source) => Err(InputError::Resolve {
            path: path.to_owned(),
            source,
        }),
    }
}
