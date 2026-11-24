use crate::{check_cancelled, error::*, workspace::*};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use mf_runtime::{ExecutionContext, InputValue, NodeValue, Shared, ValueRef};
use rustix::{
    fd::OwnedFd,
    fs::{Dir, FileType, Stat},
    io::Errno,
};
use serde::Serialize;
use snafu::{ResultExt, ensure};
use std::ops::ControlFlow::{self, Break, Continue};

const MAX_ENTRIES: usize = 50_000;
pub const MAX_DEPTH: usize = 64;
const MAX_IGNORE_BYTES: usize = 64 * 1024;
const OUTPUT_BYTES: usize = 50 * 1024;

pub struct Page<T> {
    rows: Vec<T>,
    bytes: usize,
    limit: usize,
}

impl<T: Serialize + InputValue> Page<T> {
    pub fn new(limit: usize) -> Result<Self, CodingError> {
        ensure!(
            (1..=1000).contains(&limit),
            InvalidInputSnafu {
                field: "limit",
                requirement: "1 to 1000 results"
            }
        );
        Ok(Self {
            rows: Vec::new(),
            bytes: 2,
            limit,
        })
    }

    pub fn push(&mut self, row: T) -> ControlFlow<()> {
        if self.rows.len() == self.limit {
            return Break(());
        }
        let bytes = serde_json::to_vec(&row).expect("JSON search result").len()
            + usize::from(!self.rows.is_empty());
        if self.bytes + bytes > OUTPUT_BYTES {
            return Break(());
        }
        self.rows.push(row);
        self.bytes += bytes;
        Continue(())
    }

    pub fn finish(self) -> Shared<Vec<T>> {
        Shared::new(ValueRef::from(serde_json::json!(self.rows)).into_secret())
            .expect("typed search results")
    }
}

pub struct Entry<'a> {
    pub parent: &'a OwnedFd,
    pub name: &'a str,
    pub path: &'a str,
    pub stat: Stat,
}

impl Entry<'_> {
    pub fn kind(&self) -> &'static str {
        if self.is_dir() { "directory" } else { "file" }
    }

    pub fn record(&self) -> PathEntry {
        PathEntry {
            path: self.path.to_owned(),
            kind: self.kind().to_owned(),
        }
    }

    pub fn is_dir(&self) -> bool {
        FileType::from_raw_mode(self.stat.st_mode) == FileType::Directory
    }
}

#[derive(NodeValue, Serialize)]
pub struct PathEntry {
    path: String,
    kind: String,
}

#[derive(NodeValue)]
pub struct Entries {
    pub entries: Shared<Vec<PathEntry>>,
    pub truncated: bool,
}

#[derive(Clone, Copy)]
pub enum Scope {
    FileOrTree,
    Directory { max_depth: usize },
}

type Visit = ControlFlow<()>;

struct Walker<'a, F> {
    context: &'a ExecutionContext,
    rules: Vec<Gitignore>,
    remaining: usize,
    scope: Scope,
    visit: F,
}

impl<F: FnMut(&Entry<'_>) -> Result<Visit, CodingError>> Walker<'_, F> {
    fn load_rules(&mut self, dir: &OwnedFd, prefix: &str) -> Result<(), CodingError> {
        match stat_entry(dir, ".gitignore") {
            Err(CodingError::ResolvePath {
                source: Errno::NOENT,
                ..
            }) => return Ok(()),
            Ok(stat) if !is_regular(&stat) => return Ok(()),
            result => {
                result?;
            }
        }
        // Do not let a path-based ignore loader follow a symlink or consult global Git configuration.
        let (rules, _) = read_file(dir, ".gitignore", MAX_IGNORE_BYTES)?;
        let mut builder = GitignoreBuilder::new(if prefix.is_empty() { "." } else { prefix });
        for line in rules.trim_start_matches('\u{feff}').lines() {
            builder.add_line(None, line).context(IgnoreRulesSnafu)?;
        }
        self.rules.push(builder.build().context(IgnoreRulesSnafu)?);
        Ok(())
    }

    fn ignored(&self, entry: &Entry<'_>) -> bool {
        self.rules
            .iter()
            .rev()
            .map(|rules| rules.matched(entry.path, entry.is_dir()))
            .find(|matched| !matched.is_none())
            .is_some_and(|matched| matched.is_ignore())
    }

    fn directory(
        &mut self,
        dir: &OwnedFd,
        prefix: &str,
        depth: usize,
    ) -> Result<Visit, CodingError> {
        if depth >= MAX_DEPTH {
            return Ok(Break(()));
        }
        let inherited = self.rules.len();
        self.load_rules(dir, prefix)?;
        let listing = Dir::read_from(dir).context(ResolvePathSnafu {
            operation: "list workspace directory",
        })?;
        let mut names = Vec::new();
        for item in listing {
            check_cancelled(self.context)?;
            let item = item.context(ResolvePathSnafu {
                operation: "read workspace directory entry",
            })?;
            if matches!(item.file_name().to_bytes(), b"." | b"..") {
                continue;
            }
            if self.remaining == 0 {
                return Ok(Break(()));
            }
            self.remaining -= 1;
            if let Ok(name) = item.file_name().to_str()
                && valid_component(name)
            {
                names.push(name.to_owned());
            }
        }
        names.sort_unstable();
        for name in names {
            check_cancelled(self.context)?;
            let path = join(prefix, &name);
            if path.len() > 4096 {
                continue;
            }
            let stat = match stat_entry(dir, &name) {
                Err(CodingError::ResolvePath {
                    source: Errno::NOENT,
                    ..
                }) => continue,
                result => result?,
            };
            let entry = Entry {
                parent: dir,
                name: &name,
                path: &path,
                stat,
            };
            if (!entry.is_dir() && !is_regular(&entry.stat)) || self.ignored(&entry) {
                continue;
            }
            if (self.visit)(&entry)?.is_break() {
                return Ok(Break(()));
            }
            if entry.is_dir()
                && !matches!(self.scope, Scope::Directory { max_depth } if depth + 1 >= max_depth)
                && self
                    .directory(&open_directory(dir, &name)?, &path, depth + 1)?
                    .is_break()
            {
                return Ok(Break(()));
            }
        }
        self.rules.truncate(inherited);
        Ok(Continue(()))
    }
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    }
}

/// Starts from the bound root even for scoped searches, so ancestor ignore rules still apply.
pub fn scan(
    root: &WorkspaceRoot,
    path: &str,
    scope: Scope,
    context: &ExecutionContext,
    visit: impl FnMut(&Entry<'_>) -> Result<Visit, CodingError>,
) -> Result<bool, CodingError> {
    check_cancelled(context)?;
    if let Scope::Directory { max_depth } = scope {
        ensure!(
            (1..=MAX_DEPTH).contains(&max_depth),
            InvalidInputSnafu {
                field: "max_depth",
                requirement: "1 to 64 levels below the starting directory"
            }
        );
    }
    if path != "." {
        validate_path(path)?;
    }
    let mut dir = open_root(&root.resolve(context)?)?;
    let mut walker = Walker {
        context,
        rules: Vec::new(),
        remaining: MAX_ENTRIES,
        scope,
        visit,
    };
    let mut prefix = String::new();
    if path != "." {
        let mut components = path.split('/').peekable();
        while let Some(name) = components.next() {
            check_cancelled(context)?;
            walker.load_rules(&dir, &prefix)?;
            prefix = join(&prefix, name);
            let stat = stat_entry(&dir, name)?;
            let entry = Entry {
                parent: &dir,
                name,
                path: &prefix,
                stat,
            };
            ensure!(
                entry.is_dir()
                    || (matches!(scope, Scope::FileOrTree)
                        && components.peek().is_none()
                        && is_regular(&entry.stat)),
                InvalidInputSnafu {
                    field: "path",
                    requirement: match scope {
                        Scope::FileOrTree => "a workspace directory or single-link regular file",
                        Scope::Directory { .. } => "a workspace directory",
                    }
                }
            );
            if walker.ignored(&entry) {
                return Ok(false);
            }
            if !entry.is_dir() {
                return Ok((walker.visit)(&entry)?.is_break());
            }
            dir = open_directory(&dir, name)?;
        }
    }
    Ok(walker.directory(&dir, &prefix, 0)?.is_break())
}
