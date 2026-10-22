use crate::{error::*, model::*};
use snafu::{ResultExt, ensure};
use std::{fs, path::Path, process::Command};

pub fn command(root: &Path, program: &str, args: &[&str]) -> Result<String, Error> {
    command_env(root, program, args, &[])
}

fn command_env(
    root: &Path,
    program: &str,
    args: &[&str],
    environment: &[(&str, &std::ffi::OsStr)],
) -> Result<String, Error> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(root)
        .envs(environment.iter().copied());
    if program == "git" {
        command
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_NO_LAZY_FETCH", "1");
    }
    let output = command.output().context(IoSnafu {
        action: format!("run {program}"),
    })?;
    ensure!(
        output.status.success(),
        CommandSnafu {
            program,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned()
        }
    );
    String::from_utf8(output.stdout).context(EncodingSnafu)
}

pub fn revision(root: &Path, reference: &str) -> Result<String, Error> {
    Ok(command(
        root,
        "git",
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )?
    .trim()
    .into())
}

pub fn verify_snapshot(root: &Path, commit: &str) -> Result<(), Error> {
    let top = command(root, "git", &["rev-parse", "--show-toplevel"])?;
    ensure!(
        Path::new(top.trim()).canonicalize().context(IoSnafu {
            action: "resolve worktree root"
        })? == root.canonicalize().context(IoSnafu {
            action: "resolve snapshot root"
        })?,
        InvalidSnafu {
            message: "snapshot is not an independent worktree root"
        }
    );

    ensure!(
        root.is_dir() && revision(root, "HEAD")? == commit,
        InvalidSnafu {
            message: "snapshot revision does not match the plan"
        }
    );
    ensure!(
        command(
            root,
            "git",
            &["status", "--porcelain", "--untracked-files=all"]
        )?
        .is_empty(),
        InvalidSnafu {
            message: format!("snapshot is dirty: {}", root.display())
        }
    );
    Ok(())
}

pub fn compare(
    repository: &Path,
    base: &str,
    head: &str,
    artifacts: Option<&Path>,
) -> Result<Comparison, Error> {
    compare_with_mode(repository, base, head, artifacts, ComparisonMode::MergeBase)
}

pub fn compare_with_mode(
    repository: &Path,
    base: &str,
    head: &str,
    artifacts: Option<&Path>,
    comparison_mode: ComparisonMode,
) -> Result<Comparison, Error> {
    let repository = repository.canonicalize().context(IoSnafu {
        action: "resolve repository",
    })?;
    let repository =
        Path::new(command(&repository, "git", &["rev-parse", "--show-toplevel"])?.trim())
            .to_path_buf();
    let head = revision(&repository, head)?;
    let base = revision(&repository, base)?;
    let base = match comparison_mode {
        ComparisonMode::MergeBase => command(&repository, "git", &["merge-base", &base, &head])?
            .trim()
            .to_owned(),
        ComparisonMode::Snapshot => base,
        ComparisonMode::Worktree | ComparisonMode::Commit => {
            return InvalidSnafu {
                message: "use a comparison request for worktree or commit mode",
            }
            .fail();
        }
    };
    let artifacts = artifacts
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repository.join("target/test-affected/snapshots"));
    ensure!(
        artifacts.is_absolute(),
        InvalidSnafu {
            message: "artifact directory must be absolute"
        }
    );
    fs::create_dir_all(&artifacts).context(IoSnafu {
        action: "create snapshot directory",
    })?;
    let mut roots = Vec::new();
    for commit in [&base, &head] {
        let root = artifacts.join(commit);
        if !root.exists() {
            command(
                &repository,
                "git",
                &[
                    "worktree",
                    "add",
                    "--detach",
                    root.to_str().ok_or_else(|| {
                        InvalidSnafu {
                            message: "snapshot path is not UTF-8",
                        }
                        .build()
                    })?,
                    commit,
                ],
            )?;
        }
        verify_snapshot(&root, commit)?;
        let original_common = common_dir(&repository)?;
        ensure!(
            common_dir(&root)? == original_common,
            InvalidSnafu {
                message: "snapshot belongs to another repository"
            }
        );
        roots.push(root.canonicalize().context(IoSnafu {
            action: "resolve snapshot",
        })?);
    }
    let statuses = command(
        &repository,
        "git",
        &[
            "diff",
            "--no-renames",
            "--name-status",
            "-z",
            &base,
            &head,
            "--",
        ],
    )?;
    let mut changes = Vec::new();
    let mut fields = statuses.split('\0').filter(|s| !s.is_empty());
    while let Some(status) = fields.next() {
        let path = fields.next().ok_or_else(|| {
            InvalidSnafu {
                message: "Git diff status lacks a path",
            }
            .build()
        })?;
        let patch = command(
            &repository,
            "git",
            &[
                "--literal-pathspecs",
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--no-renames",
                "--unified=0",
                &base,
                &head,
                "--",
                path,
            ],
        )?;
        let mut change = Change {
            path: path.into(),
            deleted: status == "D",
            before_lines: Vec::new(),
        };
        for line in patch.lines().filter(|line| line.starts_with("@@ ")) {
            let fields: Vec<_> = line.split_whitespace().collect();
            ensure!(
                fields.len() >= 4,
                InvalidSnafu {
                    message: "invalid Git hunk header"
                }
            );
            if let Some(range) = hunk_lines(&fields[1][1..])? {
                change.before_lines.push(range);
            }
        }
        changes.push(change);
    }
    Ok(Comparison {
        comparison_mode,
        repository,
        base,
        head,
        base_root: roots.remove(0),
        head_root: roots.remove(0),
        changes,
    })
}

fn common_dir(root: &Path) -> Result<std::path::PathBuf, Error> {
    let name = command(root, "git", &["rev-parse", "--git-common-dir"])?;
    root.join(name.trim()).canonicalize().context(IoSnafu {
        action: "resolve Git common directory",
    })
}

/// Parses a hunk header coordinate (`-start,count`) into the base-side
/// inclusive line interval. Pure insertions (`count == 0`) have no base-side
/// lines and yield no queryable range.
fn hunk_lines(value: &str) -> Result<Option<crate::graph::LineRange>, Error> {
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    let (Ok(start), Ok(count)) = (start.parse::<u32>(), count.parse::<u32>()) else {
        return InvalidSnafu {
            message: "invalid Git hunk coordinates",
        }
        .fail();
    };
    Ok((count > 0 && start >= 1).then(|| crate::graph::LineRange {
        start,
        end: start + count - 1,
    }))
}

/// Resolves local comparison modes into immutable Git snapshots.
pub fn compare_request(
    repository: &Path,
    request: &ComparisonRequest,
    artifacts: Option<&Path>,
) -> Result<Comparison, Error> {
    let repository = repository.canonicalize().context(IoSnafu {
        action: "resolve comparison repository",
    })?;
    let repository =
        Path::new(command(&repository, "git", &["rev-parse", "--show-toplevel"])?.trim())
            .to_path_buf();
    let head = revision(&repository, &request.head)?;
    if request.require_checkout {
        ensure!(
            revision(&repository, "HEAD")? == head
                && command(
                    &repository,
                    "git",
                    &["status", "--porcelain", "--untracked-files=all"]
                )?
                .is_empty(),
            InvalidSnafu {
                message: "comparison requires the clean checked-out head"
            }
        );
    }
    let (base, selected_head, mode) = match request.mode {
        ComparisonMode::Worktree => {
            ensure!(
                request.base.is_none() && head == revision(&repository, "HEAD")?,
                InvalidSnafu {
                    message: "worktree mode requires HEAD and no explicit base"
                }
            );
            (
                head.clone(),
                worktree_snapshot(&repository, &head)?,
                ComparisonMode::Snapshot,
            )
        }
        ComparisonMode::Commit => {
            ensure!(
                request.base.is_none(),
                InvalidSnafu {
                    message: "commit mode derives its first-parent base"
                }
            );
            let contents = command(&repository, "git", &["cat-file", "commit", &head])?;
            let parent = contents
                .lines()
                .take_while(|line| !line.is_empty())
                .find_map(|line| line.strip_prefix("parent "));
            let base = if let Some(parent) = parent {
                parent.to_owned()
            } else {
                let tree = command(
                    &repository,
                    "git",
                    &["hash-object", "-t", "tree", "-w", "--stdin"],
                )?;
                snapshot_commit(&repository, tree.trim(), None, &head)?
            };
            (base, head, ComparisonMode::Snapshot)
        }
        mode => (
            request.base.clone().ok_or_else(|| {
                InvalidSnafu {
                    message: "comparison requires base",
                }
                .build()
            })?,
            head,
            mode,
        ),
    };
    let mut comparison = compare_with_mode(&repository, &base, &selected_head, artifacts, mode)?;
    comparison.comparison_mode = request.mode;
    Ok(comparison)
}

fn worktree_snapshot(repository: &Path, head: &str) -> Result<String, Error> {
    let directory = tempfile::tempdir().context(IoSnafu {
        action: "prepare snapshot index",
    })?;
    let index = directory.path().join("index");
    let original =
        repository.join(command(repository, "git", &["rev-parse", "--git-path", "index"])?.trim());
    let environment = [("GIT_INDEX_FILE", index.as_os_str())];
    if original.exists() {
        fs::copy(original, &index).context(IoSnafu {
            action: "copy caller index",
        })?;
    } else {
        command_env(repository, "git", &["read-tree", head], &environment)?;
    }
    command_env(
        repository,
        "git",
        &["add", "--all", "--", "."],
        &environment,
    )?;
    let tree = command_env(repository, "git", &["write-tree"], &environment)?;
    if tree.trim()
        == command(
            repository,
            "git",
            &["rev-parse", &format!("{head}^{{tree}}")],
        )?
        .trim()
    {
        return Ok(head.into());
    }
    snapshot_commit(repository, tree.trim(), Some(head), head)
}

fn snapshot_commit(
    repository: &Path,
    tree: &str,
    parent: Option<&str>,
    clock: &str,
) -> Result<String, Error> {
    use std::ffi::OsStr;
    let date = command(
        repository,
        "git",
        &["show", "--no-patch", "--format=%cI", clock],
    )?;
    let environment = [
        ("GIT_AUTHOR_NAME", OsStr::new("Miniflow")),
        ("GIT_AUTHOR_EMAIL", OsStr::new("tools@localhost")),
        ("GIT_COMMITTER_NAME", OsStr::new("Miniflow")),
        ("GIT_COMMITTER_EMAIL", OsStr::new("tools@localhost")),
        ("GIT_AUTHOR_DATE", OsStr::new(date.trim())),
        ("GIT_COMMITTER_DATE", OsStr::new(date.trim())),
    ];
    let mut args = vec![
        "-c",
        "commit.gpgsign=false",
        "commit-tree",
        tree,
        "-m",
        "Repository tooling snapshot",
    ];
    if let Some(parent) = parent {
        args.extend(["-p", parent]);
    }
    Ok(command_env(repository, "git", &args, &environment)?
        .trim()
        .into())
}
