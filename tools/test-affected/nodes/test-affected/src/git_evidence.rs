use crate::{
    error::*,
    evidence::{Budget, EvidenceOutput},
};
use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeMetadata, NodeRegistration,
    NodeValue, PreparedNode, TypedNodeResult, TypedTaskNode, deserialize_config,
};
use serde::Deserialize;
use serde_json::{Value, json};
use snafu::{ResultExt, ensure};
use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
    process::{Child, Command, Output, Stdio},
    thread,
    time::Duration,
};

const MAX_CAPTURE: u64 = 16 * 1024 * 1024;

struct Process {
    child: Child,
    closed: bool,
}
impl Process {
    fn close(&mut self) -> Result<(), Error> {
        if self.closed {
            return Ok(());
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        if self.child.try_wait().context(CleanupSnafu)?.is_none() {
            self.child.kill().context(CleanupSnafu)?;
        }
        self.child.wait().context(CleanupSnafu)?;
        self.closed = true;
        Ok(())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn run(
    root: &Path,
    args: &[&str],
    budget: &Budget,
    context: &ExecutionContext,
) -> Result<Output, Error> {
    budget.check(context)?;
    let mut stdout = tempfile::tempfile().context(IoSnafu {
        action: "capture Git stdout",
    })?;
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .stdin(Stdio::null())
        .stdout(stdout.try_clone().context(IoSnafu {
            action: "clone Git stdout",
        })?)
        // Transport diagnostics can contain credential-bearing remote URLs.
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut process = Process {
        child: command.spawn().context(IoSnafu {
            action: "start Git evidence operation",
        })?,
        closed: false,
    };
    let result = (|| {
        let status = loop {
            budget.check(context)?;
            ensure!(
                stdout
                    .metadata()
                    .context(IoSnafu {
                        action: "measure Git stdout"
                    })?
                    .len()
                    <= MAX_CAPTURE,
                LimitSnafu {
                    message: "Git output too large"
                }
            );
            if let Some(status) = process.child.try_wait().context(IoSnafu {
                action: "poll Git evidence operation",
            })? {
                break status;
            }
            thread::sleep(Duration::from_millis(5));
        };
        stdout.seek(SeekFrom::Start(0)).context(IoSnafu {
            action: "rewind Git stdout",
        })?;
        let mut out = Vec::new();
        stdout
            .take(MAX_CAPTURE + 1)
            .read_to_end(&mut out)
            .context(IoSnafu {
                action: "read Git stdout",
            })?;
        ensure!(
            out.len() as u64 <= MAX_CAPTURE,
            LimitSnafu {
                message: "Git output too large"
            }
        );
        Ok(Output {
            status,
            stdout: out,
            stderr: Vec::new(),
        })
    })();
    process.close()?;
    result
}

fn text(
    root: &Path,
    args: &[&str],
    budget: &Budget,
    context: &ExecutionContext,
) -> Result<String, Error> {
    let output = run(root, args, budget, context)?;
    // Git transport diagnostics can include credential-bearing remote URLs.
    ensure!(
        output.status.success(),
        CommandSnafu {
            program: "git",
            status: output.status,
            stderr: "evidence operation failed"
        }
    );
    String::from_utf8(output.stdout).context(EncodingSnafu)
}

fn resolve(
    root: &Path,
    reference: &str,
    budget: &Budget,
    context: &ExecutionContext,
) -> Result<Option<String>, Error> {
    let output = run(
        root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
        budget,
        context,
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8(output.stdout)
            .context(EncodingSnafu)?
            .trim()
            .into(),
    ))
}

fn facts(
    root: &Path,
    commit: &str,
    budget: &Budget,
    context: &ExecutionContext,
) -> Result<Value, Error> {
    let contents = text(root, &["cat-file", "commit", commit], budget, context)?;
    let headers: Vec<_> = contents
        .lines()
        .take_while(|line| !line.is_empty())
        .collect();
    let tree = headers
        .iter()
        .find_map(|line| line.strip_prefix("tree "))
        .ok_or_else(|| {
            EvidenceSnafu {
                message: "commit has no tree",
            }
            .build()
        })?;
    let parents: Vec<_> = headers
        .iter()
        .filter_map(|line| line.strip_prefix("parent "))
        .collect();
    Ok(json!({"commit":commit,"tree":tree,"parents":parents}))
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct InspectConfig {
    required: bool,
    timeout_ms: u64,
    max_revisions: usize,
}
impl Default for InspectConfig {
    fn default() -> Self {
        Self {
            required: true,
            timeout_ms: 30_000,
            max_revisions: 100,
        }
    }
}
#[derive(NodeValue)]
struct InspectInput {
    repository: String,
    revisions: Option<Vec<String>>,
    ancestor_of: Option<String>,
    deadline: Option<u64>,
}
struct InspectNode {
    config: InspectConfig,
}
impl InspectNode {
    fn inspect(
        &self,
        input: &InspectInput,
        budget: &Budget,
        context: &ExecutionContext,
    ) -> Result<Value, Error> {
        let root = Path::new(&input.repository);
        let head = resolve(root, "HEAD", budget, context)?.ok_or_else(|| {
            EvidenceSnafu {
                message: "repository HEAD is unavailable",
            }
            .build()
        })?;
        let mut current = facts(root, &head, budget, context)?;
        let status = run(
            root,
            &["status", "--porcelain", "--untracked-files=all"],
            budget,
            context,
        )?;
        ensure!(
            status.status.success(),
            EvidenceSnafu {
                message: "working tree status is unavailable"
            }
        );
        current["clean"] = json!(status.stdout.is_empty());
        current["shallow"] = json!(
            text(
                root,
                &["rev-parse", "--is-shallow-repository"],
                budget,
                context
            )?
            .trim()
                == "true"
        );
        let target = match &input.ancestor_of {
            Some(reference) => {
                Some(resolve(root, reference, budget, context)?.ok_or_else(|| {
                    EvidenceSnafu {
                        message: "ancestry target is unavailable",
                    }
                    .build()
                })?)
            }
            None => None,
        };
        let mut revisions = Vec::new();
        for reference in input.revisions.iter().flatten() {
            let Some(commit) = resolve(root, reference, budget, context)? else {
                revisions.push(json!({"reference":reference,"available":false}));
                continue;
            };
            let mut value = facts(root, &commit, budget, context)?;
            value["reference"] = json!(reference);
            value["available"] = json!(true);
            if let Some(target) = &target {
                let relation = run(
                    root,
                    &["merge-base", "--is-ancestor", &commit, target],
                    budget,
                    context,
                )?;
                ensure!(
                    matches!(relation.status.code(), Some(0 | 1)),
                    EvidenceSnafu {
                        message: "ancestry query failed"
                    }
                );
                let ancestor = relation.status.success();
                value["ancestor"] = json!(ancestor);
                value["distance"] = if ancestor {
                    json!(
                        text(
                            root,
                            &["rev-list", "--count", &format!("{commit}..{target}")],
                            budget,
                            context
                        )?
                        .trim()
                        .parse::<u64>()
                        .context(NumberSnafu)?
                    )
                } else {
                    Value::Null
                };
            }
            revisions.push(value);
        }
        let value = json!({"current":current,"revisions":revisions});
        ensure!(
            serde_json::to_vec(&value)
                .context(JsonSnafu {
                    action: "measure Git evidence"
                })?
                .len()
                <= 256 * 1024,
            LimitSnafu {
                message: "Git evidence too large"
            }
        );
        budget.check(context)?;
        Ok(value)
    }
}
impl TypedTaskNode for InspectNode {
    type Input = InspectInput;
    type Output = EvidenceOutput;
    fn execute(
        &self,
        input: InspectInput,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<EvidenceOutput>, NodeExecutionError> {
        ensure!(
            input
                .revisions
                .as_ref()
                .is_none_or(|r| r.len() <= self.config.max_revisions),
            InvalidSnafu {
                message: "too many requested revisions"
            }
        );
        let budget = Budget::new(input.deadline, self.config.timeout_ms)?;
        let result = self.inspect(&input, &budget, context);
        Ok(budget.outcome(result, self.config.required)?.into())
    }
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FetchConfig {
    remote: String,
    required: bool,
    timeout_ms: u64,
}
impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            remote: "origin".into(),
            required: false,
            timeout_ms: 30_000,
        }
    }
}
#[derive(NodeValue)]
struct FetchInput {
    repository: String,
    commit: String,
    deadline: Option<u64>,
}
struct FetchNode {
    config: FetchConfig,
}
impl TypedTaskNode for FetchNode {
    type Input = FetchInput;
    type Output = EvidenceOutput;
    fn execute(
        &self,
        input: FetchInput,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<EvidenceOutput>, NodeExecutionError> {
        ensure!(
            matches!(input.commit.len(), 40 | 64)
                && input.commit.bytes().all(|b| b.is_ascii_hexdigit()),
            InvalidSnafu {
                message: "fetch requires a complete commit object ID"
            }
        );
        let root = Path::new(&input.repository);
        let budget = Budget::new(input.deadline, self.config.timeout_ms)?;
        let result = (|| {
            if resolve(root, &input.commit, &budget, context)?.is_none() {
                text(
                    root,
                    &["remote", "get-url", &self.config.remote],
                    &budget,
                    context,
                )?;
                text(
                    root,
                    &[
                        "fetch",
                        "--no-tags",
                        "--no-write-fetch-head",
                        "--no-recurse-submodules",
                        &self.config.remote,
                        &input.commit,
                    ],
                    &budget,
                    context,
                )?;
            }
            ensure!(
                resolve(root, &input.commit, &budget, context)?.is_some(),
                EvidenceSnafu {
                    message: "requested commit is unavailable after fetch"
                }
            );
            Ok(json!({"commit":input.commit}))
        })();
        Ok(budget.outcome(result, self.config.required)?.into())
    }
}
fn inspect_factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let config: InspectConfig = deserialize_config(config)?;
    ensure!(
        (1..=100).contains(&config.max_revisions),
        InvalidSnafu {
            message: "max_revisions must be in 1..=100"
        }
    );
    Budget::new(None, config.timeout_ms)?;
    PreparedNode::typed_task(InspectNode { config }, NodeMetadata::default())
}
fn fetch_factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let config: FetchConfig = deserialize_config(config)?;
    ensure!(
        !config.remote.is_empty()
            && config
                .remote
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
            && !config.remote.starts_with('-'),
        InvalidSnafu {
            message: "remote must name a configured Git remote"
        }
    );
    Budget::new(None, config.timeout_ms)?;
    PreparedNode::typed_task(FetchNode { config }, NodeMetadata::default())
}
inventory::submit! {NodeRegistration {kind:"git.inspect",factory:mf_runtime::NodeFactory::Plain(inspect_factory)}}
inventory::submit! {NodeRegistration {kind:"git.fetch",factory:mf_runtime::NodeFactory::Plain(fetch_factory)}}
