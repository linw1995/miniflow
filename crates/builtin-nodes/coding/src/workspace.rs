use crate::error::*;
use mf_runtime::{ConfigTemplate, ExecutionContext, NodeMetadata, ValueRef};
use rustix::{
    fd::OwnedFd,
    fs::{self, AtFlags, FileType, Mode, OFlags, Stat},
};
use serde_json::Value;
use snafu::{OptionExt, ResultExt, ensure};
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::CLOEXEC);
pub const STAGING_PREFIX: &str = ".mfn-coding-";

pub struct WorkspaceRoot(ConfigTemplate);

impl WorkspaceRoot {
    pub fn prepare(value: Value) -> Result<Self, CodingError> {
        let template = ConfigTemplate::parse(value).context(WorkspaceTemplateSnafu)?;
        if let Some(value) = template.literal() {
            root_path(value)?;
        }
        Ok(Self(template))
    }

    pub fn metadata(&self) -> NodeMetadata {
        NodeMetadata {
            context_references: self.0.references("workspace_root"),
            ..Default::default()
        }
    }

    pub fn resolve(&self, context: &ExecutionContext) -> Result<String, CodingError> {
        let root = self.0.resolve(context).context(WorkspaceTemplateSnafu)?;
        Ok(root_path(&root)?.to_owned())
    }
}

fn valid_component(part: &str) -> bool {
    !matches!(part, "" | "." | "..")
        && !part.eq_ignore_ascii_case(".git")
        && !part
            .get(..STAGING_PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(STAGING_PREFIX))
}

pub fn validate_path(path: &str) -> Result<(), CodingError> {
    ensure!(
        path.len() <= 4096 && !path.contains('\0') && path.split('/').all(valid_component),
        InvalidInputSnafu {
            field: "path",
            requirement: "a clean workspace-relative path outside Git metadata"
        }
    );
    Ok(())
}

pub fn open_root(root: &str) -> Result<OwnedFd, CodingError> {
    fs::open(root, DIR_FLAGS, Mode::empty())
        .map_err(io::Error::from)
        .context(IoSnafu {
            operation: "open workspace directory",
        })
}

// Resolve from an open parent so swapping an entry for a symlink cannot redirect the lookup.
pub fn open_directory(parent: &OwnedFd, name: &str) -> Result<OwnedFd, CodingError> {
    fs::openat(parent, name, DIR_FLAGS | OFlags::NOFOLLOW, Mode::empty()).context(
        ResolvePathSnafu {
            operation: "open workspace subdirectory",
        },
    )
}

pub fn stat_entry(parent: &OwnedFd, name: &str) -> Result<Stat, CodingError> {
    fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).context(ResolvePathSnafu {
        operation: "inspect workspace entry",
    })
}

pub fn read_file(
    parent: &OwnedFd,
    name: &str,
    limit: usize,
) -> Result<(String, Stat), CodingError> {
    fn check_regular(stat: &Stat) -> Result<(), CodingError> {
        ensure!(
            FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile && stat.st_nlink == 1,
            InvalidInputSnafu {
                field: "path",
                requirement: "a regular file with exactly one hard link"
            }
        );
        Ok(())
    }
    check_regular(&stat_entry(parent, name)?)?;
    let fd = fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY,
        Mode::empty(),
    )
    .context(ResolvePathSnafu {
        operation: "open workspace file",
    })?;
    let stat = fs::fstat(&fd).context(ResolvePathSnafu {
        operation: "inspect open workspace file",
    })?;
    check_regular(&stat)?;
    ensure!(stat.st_size <= limit as i64, FileTooLargeSnafu { limit });
    let mut bytes = Vec::new();
    File::from(fd)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .context(IoSnafu {
            operation: "read workspace file",
        })?;
    ensure!(bytes.len() <= limit, FileTooLargeSnafu { limit });
    let content = String::from_utf8(bytes)
        .map_err(|error| error.utf8_error())
        .context(FileEncodingSnafu)?;
    Ok((content, stat))
}

fn root_path(value: &ValueRef) -> Result<&str, CodingError> {
    value
        .as_str()
        .filter(|path| Path::new(path).is_absolute() && !path.contains('\0'))
        .context(InvalidConfigSnafu {
            field: "workspace_root",
            requirement: "an absolute directory path",
        })
}
