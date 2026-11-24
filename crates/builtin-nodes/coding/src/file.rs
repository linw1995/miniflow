use crate::{check_cancelled, error::*, private_text, text, workspace::*};
use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeFactory, NodeRegistration, NodeValue,
    PreparedNode, Shared, TypedNodeResult, TypedTaskNode, deserialize_config,
};
use rustix::{
    fd::OwnedFd,
    fs::{self, AtFlags, FlockOperation, Mode, OFlags, RenameFlags, Stat},
    io::Errno,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{OptionExt, ResultExt, ensure};
use std::{fs::File, io::Write as _};
use uuid::Uuid;

pub const READ_KIND: &str = "builtin.coding.read";
pub const EDIT_KIND: &str = "builtin.coding.edit";
pub const MKDIR_KIND: &str = "builtin.coding.mkdir";
pub const WRITE_KIND: &str = "builtin.coding.write";

const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const PAGE_BYTES: usize = 50 * 1024;
const PAGE_LINES: usize = 2_000;
const MAX_EDITS: usize = 128;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    workspace_root: Value,
    #[serde(default = "default_file_bytes")]
    max_file_bytes: usize,
}

const fn default_file_bytes() -> usize {
    DEFAULT_FILE_BYTES
}

struct WorkspaceFiles {
    root: WorkspaceRoot,
    max_file_bytes: usize,
}

struct Target<'a> {
    parent: OwnedFd,
    name: &'a str,
}

impl<'a> Target<'a> {
    fn resolve(
        root: &WorkspaceRoot,
        path: &'a str,
        context: &ExecutionContext,
    ) -> Result<Self, CodingError> {
        check_cancelled(context)?;
        Self::open(&root.resolve(context)?, path)
    }
    fn open(root: &str, path: &'a str) -> Result<Self, CodingError> {
        validate_path(path)?;
        let mut parent = open_root(root)?;
        let (directories, name) = path
            .rsplit_once('/')
            .map_or((None, path), |(dir, name)| (Some(dir), name));
        for component in directories.into_iter().flat_map(|dir| dir.split('/')) {
            parent = open_directory(&parent, component)?;
        }
        Ok(Self { parent, name })
    }

    fn lock_mutation(&self) -> Result<(), CodingError> {
        match fs::flock(&self.parent, FlockOperation::NonBlockingLockExclusive) {
            Err(Errno::WOULDBLOCK) => DirectoryBusySnafu.fail(),
            result => result.context(ResolvePathSnafu {
                operation: "lock target directory",
            }),
        }
    }

    fn read(&self, limit: usize) -> Result<(String, Stat), CodingError> {
        read_file(&self.parent, self.name, limit)
    }

    fn publish(
        &self,
        content: &str,
        original: Option<Stat>,
        context: &ExecutionContext,
    ) -> Result<(), CodingError> {
        check_cancelled(context)?;
        let staging_name = format!("{STAGING_PREFIX}{}.tmp", Uuid::new_v4());
        let mut stage = StagedFile::create(&self.parent, staging_name)?;
        stage.file.write_all(content.as_bytes()).context(IoSnafu {
            operation: "stage workspace file",
        })?;
        if let Some(original) = original {
            let permissions =
                Mode::from_raw_mode(original.st_mode) & (Mode::RWXU | Mode::RWXG | Mode::RWXO);
            fs::fchmod(&stage.file, permissions).context(ResolvePathSnafu {
                operation: "preserve workspace file permissions",
            })?;
            ensure!(
                same_revision(&original, &stat_entry(&self.parent, self.name)?),
                FileChangedSnafu
            );
        }
        check_cancelled(context)?;
        fs::renameat_with(
            &self.parent,
            &stage.name,
            &self.parent,
            self.name,
            if original.is_some() {
                RenameFlags::empty()
            } else {
                RenameFlags::NOREPLACE
            },
        )
        .context(ResolvePathSnafu {
            operation: "publish workspace file",
        })?;
        stage.published = true;
        Ok(())
    }
}

fn same_revision(left: &Stat, right: &Stat) -> bool {
    left.st_dev == right.st_dev
        && left.st_ino == right.st_ino
        && left.st_size == right.st_size
        && left.st_nlink == right.st_nlink
        && (left.st_mtime, left.st_mtime_nsec) == (right.st_mtime, right.st_mtime_nsec)
        && (left.st_ctime, left.st_ctime_nsec) == (right.st_ctime, right.st_ctime_nsec)
}

struct StagedFile<'a> {
    parent: &'a OwnedFd,
    name: String,
    file: File,
    published: bool,
}

impl<'a> StagedFile<'a> {
    fn create(parent: &'a OwnedFd, name: String) -> Result<Self, CodingError> {
        let fd = fs::openat(
            parent,
            &name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .context(ResolvePathSnafu {
            operation: "create staging file",
        })?;
        Ok(Self {
            parent,
            name,
            file: fd.into(),
            published: false,
        })
    }
}

impl Drop for StagedFile<'_> {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::unlinkat(self.parent, &self.name, AtFlags::empty());
        }
    }
}

#[derive(NodeValue)]
struct ReadInputs {
    path: Shared<String>,
    start_line: Option<usize>,
    limit: Option<usize>,
}

#[derive(NodeValue)]
struct ReadOutputs {
    content: Shared<String>,
    next_line: Option<usize>,
}

struct ReadNode(WorkspaceFiles);

impl TypedTaskNode for ReadNode {
    type Input = ReadInputs;
    type Output = ReadOutputs;

    fn execute(
        &self,
        input: ReadInputs,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<ReadOutputs>, NodeExecutionError> {
        let start = input.start_line.unwrap_or(1);
        let limit = input.limit.unwrap_or(PAGE_LINES);
        ensure!(
            start > 0,
            InvalidInputSnafu {
                field: "start_line",
                requirement: "a one-based line number"
            }
        );
        ensure!(
            (1..=PAGE_LINES).contains(&limit),
            InvalidInputSnafu {
                field: "limit",
                requirement: "1 to 2000 lines"
            }
        );
        let target = Target::resolve(&self.0.root, text(&input.path), context)?;
        let (file, _) = target.read(self.0.max_file_bytes)?;
        let mut page = String::new();
        let mut next_line = None;
        for (offset, line) in file.split_inclusive('\n').skip(start - 1).enumerate() {
            if offset == limit || page.len() + line.len() > PAGE_BYTES {
                ensure!(
                    offset > 0,
                    LineTooLongSnafu {
                        line: start,
                        limit: PAGE_BYTES
                    }
                );
                next_line = Some(start + offset);
                break;
            }
            page.push_str(line);
        }
        check_cancelled(context)?;
        let mut result = TypedNodeResult::from(ReadOutputs {
            content: private_text(page),
            next_line,
        });
        if next_line.is_none() {
            result.skipped.insert("next_line".into());
        }
        Ok(result)
    }
}

#[derive(NodeValue)]
struct MkdirInputs {
    path: Shared<String>,
}

#[derive(NodeValue)]
struct MkdirOutputs {
    created: bool,
}

struct MkdirNode(WorkspaceRoot);

impl TypedTaskNode for MkdirNode {
    type Input = MkdirInputs;
    type Output = MkdirOutputs;

    fn execute(
        &self,
        input: MkdirInputs,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<MkdirOutputs>, NodeExecutionError> {
        let target = Target::resolve(&self.0, text(&input.path), context)?;
        target.lock_mutation()?;
        check_cancelled(context)?;
        fs::mkdirat(&target.parent, target.name, Mode::RWXU).context(ResolvePathSnafu {
            operation: "create workspace directory",
        })?;
        Ok(MkdirOutputs { created: true }.into())
    }
}

#[derive(NodeValue)]
struct WriteInputs {
    path: Shared<String>,
    content: Shared<String>,
}

#[derive(NodeValue)]
struct WriteOutputs {
    bytes_written: usize,
}

struct WriteNode(WorkspaceFiles);

impl TypedTaskNode for WriteNode {
    type Input = WriteInputs;
    type Output = WriteOutputs;

    fn execute(
        &self,
        input: WriteInputs,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<WriteOutputs>, NodeExecutionError> {
        let content = text(&input.content);
        let limit = self.0.max_file_bytes;
        ensure!(content.len() <= limit, FileTooLargeSnafu { limit });
        let target = Target::resolve(&self.0.root, text(&input.path), context)?;
        target.lock_mutation()?;
        target.publish(content, None, context)?;
        Ok(WriteOutputs {
            bytes_written: content.len(),
        }
        .into())
    }
}

#[derive(NodeValue)]
struct Replacement {
    old_text: Shared<String>,
    new_text: Shared<String>,
}

#[derive(NodeValue)]
struct EditInputs {
    path: Shared<String>,
    edits: Shared<Vec<Replacement>>,
}

#[derive(NodeValue)]
struct EditOutputs {
    replacements: usize,
    bytes_written: usize,
}

struct EditNode(WorkspaceFiles);

impl TypedTaskNode for EditNode {
    type Input = EditInputs;
    type Output = EditOutputs;

    fn execute(
        &self,
        input: EditInputs,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<EditOutputs>, NodeExecutionError> {
        let count = input.edits.value().as_array().expect("typed list").len();
        ensure!(
            (1..=MAX_EDITS).contains(&count),
            InvalidInputSnafu {
                field: "edits",
                requirement: "1 to 128 replacements"
            }
        );
        let edits = input.edits.decode().context(EditDecodeSnafu)?;
        let target = Target::resolve(&self.0.root, text(&input.path), context)?;
        target.lock_mutation()?;
        let (original, revision) = target.read(self.0.max_file_bytes)?;
        let updated = apply_edits(&original, &edits, self.0.max_file_bytes)?;
        target.publish(&updated, Some(revision), context)?;
        Ok(EditOutputs {
            replacements: count,
            bytes_written: updated.len(),
        }
        .into())
    }
}

fn apply_edits(original: &str, edits: &[Replacement], limit: usize) -> Result<String, CodingError> {
    let mut ranges = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        let index = index + 1;
        let old = text(&edit.old_text);
        let new = text(&edit.new_text);
        let first = old.chars().next().context(EditMatchSnafu {
            index,
            requirement: "have nonempty old_text",
        })?;
        ensure!(
            old != new,
            EditMatchSnafu {
                index,
                requirement: "change the matched text"
            }
        );
        let start = original.find(old).context(EditMatchSnafu {
            index,
            requirement: "match text in the original file",
        })?;
        ensure!(
            !original[start + first.len_utf8()..].contains(old),
            EditMatchSnafu {
                index,
                requirement: "match exactly once in the original file"
            }
        );
        ranges.push((start, start + old.len(), new));
    }
    ranges.sort_unstable_by_key(|&(start, _, _)| start);
    ensure!(
        ranges.windows(2).all(|pair| pair[0].1 <= pair[1].0),
        OverlappingEditsSnafu
    );
    let unchanged = original.len()
        - ranges
            .iter()
            .map(|(start, end, _)| end - start)
            .sum::<usize>();
    let bytes = ranges.iter().fold(unchanged, |total, (_, _, replacement)| {
        total.saturating_add(replacement.len())
    });
    ensure!(bytes <= limit, FileTooLargeSnafu { limit });
    let mut result = String::with_capacity(bytes);
    let mut offset = 0;
    for (start, end, replacement) in ranges {
        result.push_str(&original[offset..start]);
        result.push_str(replacement);
        offset = end;
    }
    result.push_str(&original[offset..]);
    Ok(result)
}

fn prepare<N: TypedTaskNode + 'static>(
    value: Value,
    node: impl FnOnce(WorkspaceFiles) -> N,
) -> Result<PreparedNode, NodeBuildError> {
    let config: Config = deserialize_config(value)?;
    ensure!(
        (1..=MAX_FILE_BYTES).contains(&config.max_file_bytes),
        InvalidConfigSnafu {
            field: "max_file_bytes",
            requirement: "1 to 16777216 bytes"
        }
    );
    let root = WorkspaceRoot::prepare(config.workspace_root)?;
    let metadata = root.metadata();
    PreparedNode::typed_task(
        node(WorkspaceFiles {
            root,
            max_file_bytes: config.max_file_bytes,
        }),
        metadata,
    )
}

inventory::submit! {
    NodeRegistration { kind: READ_KIND, factory: NodeFactory::Plain(|value| prepare(value, ReadNode)) }
}
inventory::submit! {
    NodeRegistration { kind: EDIT_KIND, factory: NodeFactory::Plain(|value| prepare(value, EditNode)) }
}
inventory::submit! {
    NodeRegistration { kind: MKDIR_KIND, factory: NodeFactory::Plain(|value| prepare_root_node(value, MkdirNode)) }
}
inventory::submit! {
    NodeRegistration { kind: WRITE_KIND, factory: NodeFactory::Plain(|value| prepare(value, WriteNode)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_does_not_replace_an_externally_changed_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("file");
        std::fs::write(&path, "before").unwrap();
        let target = Target::open(root.path().to_str().unwrap(), "file").unwrap();
        target.lock_mutation().unwrap();
        let (_, revision) = target.read(64).unwrap();
        std::fs::write(&path, "changed outside the tool").unwrap();
        assert!(matches!(
            target.publish("planned edit", Some(revision), &ExecutionContext::default()),
            Err(CodingError::FileChanged)
        ));
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "changed outside the tool"
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
