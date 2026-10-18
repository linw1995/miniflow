use crate::{error::*, model::*};
use mfn_lsp::{AnalysisRequest, Document, Snapshot, SourceRange};
use serde_json::Value;
use snafu::{ResultExt, ensure};
use std::{fs, path::Path, process::Command};

pub fn command(root: &Path, program: &str, args: &[&str]) -> Result<String, Error> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .context(IoSnafu {
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
    options: Value,
) -> Result<Comparison, Error> {
    compare_with_mode(
        repository,
        base,
        head,
        artifacts,
        options,
        ComparisonMode::MergeBase,
    )
}

pub fn compare_with_mode(
    repository: &Path,
    base: &str,
    head: &str,
    artifacts: Option<&Path>,
    options: Value,
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
    let paths = command(
        &repository,
        "git",
        &[
            "diff",
            "--no-renames",
            "--name-only",
            "-z",
            &base,
            &head,
            "--",
        ],
    )?;
    let mut changes = Vec::new();
    for path in paths.split('\0').filter(|s| !s.is_empty()) {
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
        let before = fs::read(roots[0].join(path))
            .or_else(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(Vec::new())
                } else {
                    Err(e)
                }
            })
            .context(IoSnafu {
                action: format!("read base {path}"),
            })?;
        let after = fs::read(roots[1].join(path))
            .or_else(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(Vec::new())
                } else {
                    Err(e)
                }
            })
            .context(IoSnafu {
                action: format!("read head {path}"),
            })?;
        let mut change = Change {
            path: path.into(),
            before: Vec::new(),
            after: Vec::new(),
        };
        for line in patch.lines().filter(|line| line.starts_with("@@ ")) {
            let fields: Vec<_> = line.split_whitespace().collect();
            ensure!(
                fields.len() >= 4,
                InvalidSnafu {
                    message: "invalid Git hunk header"
                }
            );
            change.before.push(hunk_range(&before, &fields[1][1..])?);
            change.after.push(hunk_range(&after, &fields[2][1..])?);
        }
        changes.push(change);
    }
    let snapshots = [
        (&base, "base", &roots[0], true),
        (&head, "head", &roots[1], false),
    ]
    .into_iter()
    .map(|(id, role, root, before)| Snapshot {
        id: format!("{role}:{id}"),
        root: root.clone(),
        documents: changes
            .iter()
            .filter(|c| root.join(&c.path).is_file())
            .map(|c| Document {
                path: c.path.clone(),
                ranges: if before {
                    c.before.clone()
                } else {
                    c.after.clone()
                },
            })
            .collect(),
    })
    .collect();
    Ok(Comparison {
        comparison_mode,
        repository,
        base,
        head,
        base_root: roots.remove(0),
        head_root: roots.remove(0),
        changes,
        request: AnalysisRequest {
            snapshots,
            initialization_options: options,
        },
    })
}

fn common_dir(root: &Path) -> Result<std::path::PathBuf, Error> {
    let name = command(root, "git", &["rev-parse", "--git-common-dir"])?;
    root.join(name.trim()).canonicalize().context(IoSnafu {
        action: "resolve Git common directory",
    })
}

fn hunk_range(source: &[u8], value: &str) -> Result<SourceRange, Error> {
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    let (Ok(start), Ok(count)) = (start.parse::<usize>(), count.parse::<usize>()) else {
        return InvalidSnafu {
            message: "invalid Git hunk coordinates",
        }
        .fail();
    };
    let mut offsets = vec![0];
    offsets.extend(
        source
            .iter()
            .enumerate()
            .filter(|(_, byte)| **byte == b'\n')
            .map(|(index, _)| index + 1),
    );
    let first = if count == 0 {
        start
    } else {
        start.saturating_sub(1)
    };
    let from = offsets.get(first).copied().unwrap_or(source.len());
    let end = if count == 0 {
        from
    } else {
        offsets
            .get(first.saturating_add(count))
            .copied()
            .unwrap_or(source.len())
    };
    Ok(SourceRange { start: from, end })
}
