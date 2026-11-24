use crate::{
    discovery::{Entries, MAX_DEPTH, Page, Scope, scan},
    error::*,
    workspace::*,
};
use globset::{GlobBuilder, GlobSet};
use mf_runtime::{
    ExecutionContext, NodeExecutionError, NodeFactory, NodeRegistration, NodeValue,
    TypedNodeResult, TypedTaskNode,
};
use snafu::{ResultExt, ensure};
use std::{ops::ControlFlow::Continue, path::Path};

pub const FIND_KIND: &str = "builtin.coding.find";

#[derive(NodeValue)]
struct FindInputs {
    pattern: String,
    path: Option<String>,
    kind: Option<String>,
    max_depth: Option<usize>,
    limit: Option<usize>,
}

struct Find(WorkspaceRoot);

impl TypedTaskNode for Find {
    type Input = FindInputs;
    type Output = Entries;

    fn execute(
        &self,
        input: FindInputs,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Entries>, NodeExecutionError> {
        let pattern = &input.pattern;
        ensure!(
            (1..=4096).contains(&pattern.len())
                && !pattern.starts_with('/')
                && !pattern.contains('\0'),
            InvalidInputSnafu {
                field: "pattern",
                requirement: "a relative glob of 1 to 4096 UTF-8 bytes"
            }
        );
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .context(GlobSnafu)?;
        let matcher = GlobSet::new([&glob]).context(GlobSnafu)?;
        let path = input.path.as_deref().unwrap_or(".");
        let kind = input.kind.as_deref().unwrap_or("any");
        ensure!(
            matches!(kind, "any" | "file" | "directory"),
            InvalidInputSnafu {
                field: "kind",
                requirement: "any, file, or directory"
            }
        );
        let mut page = Page::new(input.limit.unwrap_or(1000))?;
        let basename = !pattern.contains('/');
        let scope = Scope::Directory {
            max_depth: input.max_depth.unwrap_or(MAX_DEPTH),
        };
        let truncated = scan(&self.0, path, scope, context, |entry| {
            let candidate = if basename {
                Path::new(entry.name)
            } else if path == "." {
                Path::new(entry.path)
            } else {
                Path::new(entry.path)
                    .strip_prefix(path)
                    .expect("entry below starting directory")
            };
            Ok(
                if (kind == "any" || kind == entry.kind()) && matcher.is_match(candidate) {
                    page.push(entry.record())
                } else {
                    Continue(())
                },
            )
        })?;
        Ok(Entries {
            entries: page.finish(),
            truncated,
        }
        .into())
    }
}

inventory::submit! {
    NodeRegistration { kind: FIND_KIND, factory: NodeFactory::Plain(|value| prepare_root_node(value, Find)) }
}
