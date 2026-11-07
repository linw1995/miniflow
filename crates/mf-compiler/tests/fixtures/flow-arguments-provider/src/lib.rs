use mf_runtime::NodeConfig;
use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodePortContract, NodePorts, NodeRegistration, NodeResult, PortSpec, PreparedNode,
    PreparedSubgraph, TaskNode, ValueType, WorkflowArguments,
};
use serde_json::Value;
use snafu::{OptionExt, ensure};
use std::collections::BTreeMap;

pub const KIND: &str = "fixture.choose_flow";

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    left: Option<PreparedSubgraph>,
    #[serde(default)]
    right: Option<PreparedSubgraph>,
}

struct ChooseFlow {
    id: String,
    arguments: BTreeMap<String, PreparedSubgraph>,
    pointer_selection: bool,
}

impl NodePortContract for ChooseFlow {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("select", ValueType::String, true)],
            outputs: vec![PortSpec::new("content", ValueType::String, true)],
        }
    }
}

impl TaskNode for ChooseFlow {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let selection = inputs["select"].as_str().expect("validated string port");
        let body = self.arguments.get(selection).with_context(|| {
            mf_runtime::NodeExecutionFailedSnafu {
                message: format!("unknown local Flow binding `{selection}`"),
            }
        })?;
        let binding = if self.pointer_selection {
            selection.to_owned()
        } else {
            format!("/{selection}")
        };
        let outputs = context.run_call(
            &body.scope_key(&self.id, &binding),
            WorkflowArguments::default(),
            &body.nodes,
            |child| body.execute_in_context(child),
        )?;
        Ok(outputs.into())
    }
}

fn plain(config: Value) -> Result<PreparedNode, NodeBuildError> {
    prepared("", config, BTreeMap::new())
}

fn prepared(
    id: &str,
    config: Value,
    arguments: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    let config = Config::from_config(config, arguments)?;
    let arguments: BTreeMap<_, _> = [("left", config.left), ("right", config.right)]
        .into_iter()
        .filter_map(|(name, body)| body.map(|body| (name.into(), body)))
        .collect();
    for body in arguments.values() {
        ensure!(
            body.inputs.inputs.values().all(BTreeMap::is_empty)
                && body.inputs.stdin.is_empty()
                && body.stream_outputs.is_empty()
                && body.outputs == vec![PortSpec::new("content", ValueType::String, true)],
            mf_runtime::NodeInvalidSubgraphSnafu {
                message: "expected a terminal string-content body without inputs"
            }
        );
    }
    PreparedNode::new(
        ChooseFlow {
            id: id.into(),
            arguments,
            pointer_selection: false,
        },
        NodeMetadata::default(),
    )
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        factory: NodeFactory::Subgraph { flow_parameters: Config::flow_parameters, plain, prepared },
    }
}

// This second receiver exercises actual Rust type traversal, not a hand-authored path list.
pub const NESTED_KIND: &str = "fixture.choose_nested_flow";

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct NestedConfig {
    #[serde(default)]
    steps: Vec<Step>,
    #[serde(default)]
    handlers: BTreeMap<String, PreparedSubgraph>,
    policy: Policy,
    #[serde(default)]
    opaque: Value,
    #[serde(default)]
    ordinary: Ordinary,
}

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct Step {
    body: PreparedSubgraph,
}

#[derive(NodeConfig)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Policy {
    Run { body: Box<PreparedSubgraph> },
    Skip { data: Value },
}

// Public NodeConfig must retain the ordinary Deserialize fallback without a parallel source type.
#[derive(Default, serde::Deserialize)]
struct Ordinary {
    #[serde(default)]
    payload: Value,
}

fn nested_plain(config: Value) -> Result<PreparedNode, NodeBuildError> {
    nested_prepared("", config, BTreeMap::new())
}

fn nested_prepared(
    id: &str,
    config: Value,
    arguments: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    let config = NestedConfig::from_config(config, arguments)?;
    let mut arguments = BTreeMap::new();
    for (index, step) in config.steps.into_iter().enumerate() {
        arguments.insert(format!("/steps/{index}/body"), step.body);
    }
    for (name, body) in config.handlers {
        let escaped = name.replace('~', "~0").replace('/', "~1");
        arguments.insert(format!("/handlers/{escaped}"), body);
    }
    match config.policy {
        Policy::Run { body } => {
            arguments.insert("/policy/body".into(), *body);
        }
        Policy::Skip { data } => {
            let _ = data;
        }
    }
    let _opaque = (config.opaque, config.ordinary.payload);
    for body in arguments.values() {
        ensure!(
            body.inputs.inputs.values().all(BTreeMap::is_empty)
                && body.inputs.stdin.is_empty()
                && body.stream_outputs.is_empty()
                && body.outputs == vec![PortSpec::new("content", ValueType::String, true)],
            mf_runtime::NodeInvalidSubgraphSnafu {
                message: "expected a terminal string-content body without inputs"
            }
        );
    }
    PreparedNode::new(
        ChooseFlow {
            id: id.into(),
            arguments,
            pointer_selection: true,
        },
        NodeMetadata::default(),
    )
}

inventory::submit! {
    NodeRegistration {
        kind: NESTED_KIND,
        factory: NodeFactory::Subgraph {
            flow_parameters: NestedConfig::flow_parameters,
            plain: nested_plain,
            prepared: nested_prepared,
        },
    }
}

fn nested_builtin() -> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError> {
    mf_runtime::WorkflowDefinition::from_json(
        r#"{
        "version":"2026-10-28", "dependencies":{},
        "nodes":[{"id":"leaf", "kind":"builtin.constant", "config":{"value":"builtin"}}],
        "outputs":[{"name":"content", "node":"leaf", "port":"value"}]
    }"#,
    )
}

inventory::submit! {
    mf_runtime::BuiltinFlowRegistration::new(
        "fixture-flow-arguments-provider", "nested", nested_builtin,
    )
}
