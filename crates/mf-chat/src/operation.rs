use crate::{ChatError, error::*};
use mf_runtime::{FlowRuntime, NodeRegistry, NodeValue, WorkflowArguments, WorkflowDefinition};
use snafu::{OptionExt, ResultExt};
use std::path::Path;

pub fn execute<I: NodeValue, O: NodeValue>(
    path: &Path,
    kind: &str,
    input: I,
) -> Result<O, ChatError> {
    let mut values = input.into_values()?;
    values.insert(
        "database_path".into(),
        path.to_str().context(PathSnafu { path })?.into(),
    );
    let outputs: Vec<_> = O::ports().into_iter().map(|port| serde_json::json!({
        "name": port.name, "node": "operation", "port": port.name, "optional": !port.required,
    })).collect();
    let definition: WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "version": "2026-10-03", "dependencies": {},
        "nodes": [{"id": "operation", "kind": kind, "config": {}}],
        "outputs": outputs,
    }))
    .context(JsonSnafu)?;
    let registry = NodeRegistry::from_inventory()?;
    let plan = mf_compiler::plan_definition(&definition)?;
    let flow = mf_compiler::instantiate_compiled(&plan, &registry)?;
    let arguments = WorkflowArguments(
        values
            .into_iter()
            .map(|(name, value)| (format!("operation.{name}"), value))
            .collect(),
    );
    let result = FlowRuntime::default().execute_with_inputs(&flow, arguments)?;
    Ok(O::from_values(result)?)
}
