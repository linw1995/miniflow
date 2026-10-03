use crate::{
    ControlEdgeDefinition, DefinitionId, EdgeDefinition, FlowNode, NodeDefinition, ValueType,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const ITERATION_KIND: &str = "builtin.iteration";
pub const ITERATION_INPUT_KIND: &str = "builtin.iteration_input";
pub const ITERATION_INPUT_ID: &str = "%iteration";

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IterationMode {
    #[default]
    Sequential,
    Parallel,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IterationErrorPolicy {
    #[default]
    Terminate,
    ContinueOnError,
    RemoveFailed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IterationConfig {
    #[serde(default)]
    pub mode: IterationMode,
    #[serde(default)]
    pub on_error: IterationErrorPolicy,
    pub body: IterationBodyDefinition,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IterationBodyDefinition {
    pub nodes: Vec<NodeDefinition>,
    #[serde(default)]
    pub edges: Vec<EdgeDefinition>,
    #[serde(default)]
    pub control_edges: Vec<ControlEdgeDefinition>,
    pub result: IterationResultDefinition,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IterationResultDefinition {
    pub node: DefinitionId,
    pub port: String,
}

pub fn iteration_input_flow_node() -> FlowNode {
    crate::prepared_scope_source(
        ITERATION_INPUT_ID,
        &BTreeMap::from([
            ("item".into(), ValueType::Any),
            ("key".into(), ValueType::Any),
        ]),
    )
}
