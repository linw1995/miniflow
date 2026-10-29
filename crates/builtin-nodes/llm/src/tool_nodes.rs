use crate::{CallOutcome, ToolCall, ToolDefinition, ToolResult, tools};
use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeMetadata, NodeRegistration,
    NodeValue, PreparedNode, TypedNodeResult, TypedTaskNode, deserialize_config,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

#[derive(Debug, Snafu)]
pub enum ToolDispatchError {
    #[snafu(display("tool catalog metadata is unavailable in this execution scope"))]
    CatalogUnavailable,
    #[snafu(transparent)]
    Configuration { source: ToolConfigError },
    #[snafu(display("tool dispatch {resource} limit exceeded"))]
    Limit { resource: &'static str },
    #[snafu(display("tool dispatch requires unique nonblank call IDs and valid names"))]
    Identity,
    #[snafu(display("tool dispatch timeout is outside the monotonic clock range"))]
    DeadlineRange,
    #[snafu(transparent)]
    Schema { source: tools::ToolSchemaError },
    #[snafu(display("tool dispatch stopped: {source}"))]
    Invocation {
        source: mf_runtime::CallDispatchError,
    },
    #[snafu(transparent)]
    Control {
        source: mf_runtime::FutureExecutionError,
    },
}

impl From<ToolDispatchError> for NodeExecutionError {
    fn from(source: ToolDispatchError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

#[derive(Debug, Snafu)]
pub enum ToolConfigError {
    #[snafu(display("tool configuration `{field}`: {source}"))]
    Template {
        field: &'static str,
        source: mf_runtime::ConfigTemplateError,
    },
    #[snafu(display("tool configuration `{field}`: {source}"))]
    Decode {
        field: &'static str,
        source: mf_runtime::InputDecodeError,
    },
    #[snafu(display("tool configuration `{field}` requires {requirement}"))]
    Invalid {
        field: &'static str,
        requirement: &'static str,
    },
}

impl From<ToolConfigError> for NodeBuildError {
    fn from(source: ToolConfigError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

#[derive(NodeValue)]
struct CatalogInputs {}

#[derive(NodeValue)]
struct CatalogOutputs {
    tools: Vec<ToolDefinition>,
}

struct CatalogNode;

impl TypedTaskNode for CatalogNode {
    type Input = CatalogInputs;
    type Output = CatalogOutputs;

    fn execute(
        &self,
        _: CatalogInputs,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<CatalogOutputs>, NodeExecutionError> {
        ensure!(!context.call_targets().is_empty(), CatalogUnavailableSnafu);
        let tools = tools::ToolCatalog::new(context)
            .map_err(ToolDispatchError::from)?
            .definitions()
            .into_iter()
            .map(|tool| ToolDefinition {
                name: tool.name,
                description: tool.description,
                input_schema: tool.parameters.into(),
            })
            .collect();
        Ok(CatalogOutputs { tools }.into())
    }
}

pub fn prepare_tool_catalog(config: Value) -> Result<PreparedNode, NodeBuildError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {
        source: mf_runtime::DefinitionId,
    }
    let config: Config = deserialize_config(config)?;
    ensure!(
        !config.source.as_str().trim().is_empty(),
        InvalidSnafu {
            field: "source",
            requirement: "a call-owning node in the same scope",
        }
    );
    PreparedNode::typed_task(
        CatalogNode,
        NodeMetadata {
            call_interface_source: Some(config.source),
            ..Default::default()
        },
    )
}

#[derive(Clone, Debug, NodeValue)]
pub struct ToolDispatchInputs {
    pub outcome: CallOutcome,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug, NodeValue)]
pub struct ToolDispatchOutputs {
    pub tool_results: Vec<ToolResult>,
}

struct DispatchNode {
    limit: mf_runtime::ConfigTemplate,
    timeout: mf_runtime::ConfigTemplate,
    argument_outputs: Vec<String>,
}

impl TypedTaskNode for DispatchNode {
    type Input = ToolDispatchInputs;
    type Output = ToolDispatchOutputs;

    fn execute(
        &self,
        inputs: ToolDispatchInputs,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<ToolDispatchOutputs>, NodeExecutionError> {
        let mut result: TypedNodeResult<ToolDispatchOutputs> =
            self.dispatch(inputs, context)?.into();
        result.skipped.extend(self.argument_outputs.iter().cloned());
        Ok(result)
    }
}

impl DispatchNode {
    fn dispatch(
        &self,
        inputs: ToolDispatchInputs,
        context: &mut ExecutionContext,
    ) -> Result<ToolDispatchOutputs, ToolDispatchError> {
        let mut results = Vec::new();
        if inputs.outcome != CallOutcome::ToolCalls {
            return Ok(ToolDispatchOutputs {
                tool_results: results,
            });
        }
        let started = Instant::now();
        let limit = resolve_setting(&self.limit, "max_tool_calls", context)?;
        let timeout = resolve_setting(&self.timeout, "timeout_ms", context)?;
        let deadline = started
            .checked_add(Duration::from_millis(timeout))
            .context(DeadlineRangeSnafu)?;
        context.check_future_limits(Some(deadline))?;
        let catalog = tools::ToolCatalog::new(context)?;
        ensure!(
            inputs.tool_calls.len() as u64 <= limit,
            LimitSnafu {
                resource: "call count"
            }
        );
        let mut ids = BTreeSet::new();
        let mut bytes = 0usize;
        // Admit the whole batch before any side effect, even when calls come from another node.
        for call in &inputs.tool_calls {
            ensure!(
                !call.id.trim().is_empty()
                    && ids.insert(&call.id)
                    && crate::call::valid_name(&call.name),
                IdentitySnafu
            );
            let encoded = tools::bounded_json(&call.arguments, tools::MAX_TOOL_ARGUMENT_BYTES)
                .ok()
                .context(LimitSnafu {
                    resource: "argument bytes",
                })?;
            bytes = bytes.saturating_add(encoded.len());
            ensure!(
                call.id.len() <= 256 && bytes <= tools::MAX_TOOL_BATCH_ARGUMENT_BYTES,
                LimitSnafu {
                    resource: "batch arguments or identity"
                }
            );
        }
        for call in inputs.tool_calls {
            context.check_future_limits(Some(deadline))?;
            let error = |message: &str| serde_json::json!({"error":message}).to_string();
            let content = match catalog.arguments(&call.name, &call.arguments) {
                Err(tools::ToolArgumentsError::UnknownTool) => error("unknown tool name"),
                Err(_) => error("arguments do not match the tool contract"),
                Ok((binding, arguments)) => match context
                    .invoke_call_with_deadline(binding, arguments, Some(deadline))
                    .context(InvocationSnafu)?
                    .outcome
                {
                    mf_runtime::CallOutcome::Succeeded(outputs) => {
                        match tools::tool_result(outputs) {
                            Ok(content) => content,
                            Err(tools::ToolEncodingError::Size) => {
                                error("tool result exceeds the size limit")
                            }
                            Err(_) => error("tool result could not be encoded"),
                        }
                    }
                    mf_runtime::CallOutcome::Skipped { .. } => {
                        error("tool was skipped because a prerequisite was unavailable")
                    }
                    mf_runtime::CallOutcome::Failed { .. } => error("tool execution failed"),
                },
            };
            results.push(ToolResult {
                tool_call_id: call.id,
                content,
            });
        }
        Ok(ToolDispatchOutputs {
            tool_results: results,
        })
    }
}

pub fn prepare_tool_dispatch(config: Value) -> Result<PreparedNode, NodeBuildError> {
    fn default_limit() -> Value {
        32.into()
    }
    fn default_timeout() -> Value {
        60_000.into()
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {
        #[serde(default)]
        outputs: BTreeMap<String, mf_runtime::ValueType>,
        #[serde(default = "default_limit")]
        max_tool_calls: Value,
        #[serde(default = "default_timeout")]
        timeout_ms: Value,
    }
    let config: Config = deserialize_config(config)?;
    let limit = prepare_setting(config.max_tool_calls, "max_tool_calls")?;
    let timeout = prepare_setting(config.timeout_ms, "timeout_ms")?;
    let context_references = limit
        .references("max_tool_calls")
        .into_iter()
        .chain(timeout.references("timeout_ms"))
        .collect();
    let mut prepared = PreparedNode::typed_task(
        DispatchNode {
            limit,
            timeout,
            argument_outputs: config.outputs.keys().cloned().collect(),
        },
        NodeMetadata {
            context_references,
            ..Default::default()
        },
    )?;
    for (name, value_type) in config.outputs {
        ensure!(
            !name.is_empty()
                && !prepared
                    .metadata
                    .output_ports()
                    .any(|port| port.name == name),
            InvalidSnafu {
                field: "outputs",
                requirement: "nonempty names distinct from protocol outputs"
            }
        );
        prepared
            .metadata
            .ports
            .outputs
            .push(mf_runtime::PortSpec::owned(name, value_type, false));
    }
    prepared.metadata.typed_generation = None;
    Ok(prepared)
}

fn validate_setting(
    field: &'static str,
    value: &mf_runtime::ValueRef,
) -> Result<u64, ToolConfigError> {
    let value = <u64 as mf_runtime::InputField>::decode_field(field, Some(value.clone()))
        .context(DecodeSnafu { field })?;
    ensure!(
        value > 0,
        InvalidSnafu {
            field,
            requirement: "a positive integer"
        }
    );
    ensure!(
        field != "max_tool_calls" || value <= tools::MAX_TOOL_CALLS as u64,
        InvalidSnafu {
            field,
            requirement: "an integer from 1 to 128"
        }
    );
    Ok(value)
}

fn prepare_setting(
    value: Value,
    field: &'static str,
) -> Result<mf_runtime::ConfigTemplate, ToolConfigError> {
    let template = mf_runtime::ConfigTemplate::parse(value).context(TemplateSnafu { field })?;
    if let Some(value) = template.literal() {
        validate_setting(field, value)?;
    }
    Ok(template)
}

fn resolve_setting(
    template: &mf_runtime::ConfigTemplate,
    field: &'static str,
    context: &ExecutionContext,
) -> Result<u64, ToolConfigError> {
    let value = template.resolve(context).context(TemplateSnafu { field })?;
    validate_setting(field, &value)
}

inventory::submit! { NodeRegistration { kind: "builtin.tool_catalog", factory: mf_runtime::NodeFactory::Plain(prepare_tool_catalog) } }
inventory::submit! { NodeRegistration { kind: "builtin.tool_dispatch", factory: mf_runtime::NodeFactory::Plain(prepare_tool_dispatch) } }

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dispatch_configuration_reuses_numeric_codecs_and_template_dependencies() {
        assert_eq!(
            validate_setting("max_tool_calls", &json!(3.0).into()).unwrap(),
            3
        );
        for config in [
            json!({"max_tool_calls":0}),
            json!({"max_tool_calls":129}),
            json!({"max_tool_calls":1.5}),
            json!({"timeout_ms":0}),
            json!({"outputs":{"tool_results":"array"}}),
        ] {
            assert!(prepare_tool_dispatch(config).is_err());
        }
        let node = prepare_tool_dispatch(json!({"max_tool_calls":{"$ref":"settings.limit"},"timeout_ms":{"$ref":"settings.timeout"}})).unwrap();
        assert_eq!(node.metadata.context_references.len(), 2);
        assert!(
            node.metadata
                .context_references
                .iter()
                .any(|reference| reference.output == "settings.limit")
        );
        assert!(
            node.metadata
                .context_references
                .iter()
                .any(|reference| reference.output == "settings.timeout")
        );
        assert!(prepare_tool_catalog(json!({"source":" "})).is_err());
        let node = prepare_tool_catalog(json!({"source":"dispatch"})).unwrap();
        assert!(
            node.execution
                .as_task_node()
                .unwrap()
                .execute(Default::default(), &mut ExecutionContext::default())
                .is_err()
        );
    }
}
