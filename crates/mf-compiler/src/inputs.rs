use crate::WorkflowDefinition;
use snafu::{ResultExt, Snafu};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Snafu)]
pub enum InputError {
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
        Ok(Self { definition, lock })
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
