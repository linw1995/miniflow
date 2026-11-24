use crate::{
    discovery::{Entries, Page, Scope, scan},
    workspace::{WorkspaceRoot, prepare_root_node},
};
use mf_runtime::{
    ExecutionContext, NodeExecutionError, NodeFactory, NodeRegistration, NodeValue,
    TypedNodeResult, TypedTaskNode,
};

pub const LS_KIND: &str = "builtin.coding.ls";

#[derive(NodeValue)]
struct LsInputs {
    path: Option<String>,
    limit: Option<usize>,
}

struct Ls(WorkspaceRoot);

impl TypedTaskNode for Ls {
    type Input = LsInputs;
    type Output = Entries;

    fn execute(
        &self,
        input: LsInputs,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Entries>, NodeExecutionError> {
        let mut page = Page::new(input.limit.unwrap_or(500))?;
        let truncated = scan(
            &self.0,
            input.path.as_deref().unwrap_or("."),
            Scope::Directory { max_depth: 1 },
            context,
            |entry| Ok(page.push(entry.record())),
        )?;
        Ok(Entries {
            entries: page.finish(),
            truncated,
        }
        .into())
    }
}

inventory::submit! {
    NodeRegistration { kind: LS_KIND, factory: NodeFactory::Plain(|value| prepare_root_node(value, Ls)) }
}
