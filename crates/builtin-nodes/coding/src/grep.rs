use crate::{
    check_cancelled,
    discovery::{Page, Scope, scan},
    error::*,
    workspace::*,
};
use mf_runtime::{
    ExecutionContext, NodeExecutionError, NodeFactory, NodeRegistration, NodeValue, Shared,
    TypedNodeResult, TypedTaskNode,
};
use regex::RegexBuilder;
use serde::Serialize;
use snafu::{ResultExt, ensure};
use std::ops::ControlFlow::{Break, Continue};

pub const GREP_KIND: &str = "builtin.coding.grep";
const SCAN_BYTES: usize = 64 * 1024 * 1024;

#[derive(NodeValue)]
struct GrepInputs {
    pattern: String,
    path: Option<String>,
    literal: Option<bool>,
    ignore_case: Option<bool>,
    limit: Option<usize>,
}

#[derive(NodeValue, Serialize)]
struct Hit {
    path: String,
    line: usize,
    text: String,
}

#[derive(NodeValue)]
struct GrepOutputs {
    matches: Shared<Vec<Hit>>,
    truncated: bool,
}

struct Grep(WorkspaceRoot);

impl TypedTaskNode for Grep {
    type Input = GrepInputs;
    type Output = GrepOutputs;

    fn execute(
        &self,
        input: GrepInputs,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<GrepOutputs>, NodeExecutionError> {
        ensure!(
            (1..=4096).contains(&input.pattern.len()),
            InvalidInputSnafu {
                field: "pattern",
                requirement: "1 to 4096 UTF-8 bytes"
            }
        );
        let pattern = if input.literal.unwrap_or(false) {
            regex::escape(&input.pattern)
        } else {
            input.pattern
        };
        let re = RegexBuilder::new(&pattern)
            .case_insensitive(input.ignore_case.unwrap_or(false))
            .size_limit(2 * 1024 * 1024)
            .build()
            .context(RegexSnafu)?;
        let mut page = Page::new(input.limit.unwrap_or(100))?;
        let mut remaining = SCAN_BYTES;
        let truncated = scan(
            &self.0,
            input.path.as_deref().unwrap_or("."),
            Scope::FileOrTree,
            context,
            |entry| {
                if entry.is_dir() || entry.stat.st_size > DEFAULT_FILE_BYTES as i64 {
                    return Ok(Continue(()));
                }
                if entry.stat.st_size > remaining as i64 {
                    return Ok(Break(()));
                }
                let limit = DEFAULT_FILE_BYTES.min(remaining);
                let (bytes, _) = read_bytes(entry.parent, entry.name, limit)?;
                remaining -= bytes.len();
                if !bytes.contains(&0)
                    && let Ok(content) = std::str::from_utf8(&bytes)
                {
                    for (offset, line) in content.lines().enumerate() {
                        if offset % 256 == 0 {
                            check_cancelled(context)?;
                        }
                        if re.is_match(line)
                            && page
                                .push(Hit {
                                    path: entry.path.to_owned(),
                                    line: offset + 1,
                                    text: line.to_owned(),
                                })
                                .is_break()
                        {
                            return Ok(Break(()));
                        }
                    }
                }
                Ok(Continue(()))
            },
        )?;
        Ok(GrepOutputs {
            matches: page.finish(),
            truncated,
        }
        .into())
    }
}

inventory::submit! {
    NodeRegistration { kind: GREP_KIND, factory: NodeFactory::Plain(|value| prepare_root_node(value, Grep)) }
}
