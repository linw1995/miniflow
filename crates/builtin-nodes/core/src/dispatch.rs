use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeInputs, NodeMetadata,
    NodePortContract, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, PreparedSubgraph,
    PreparedSubgraphs, TaskNode, ValueRef, ValueSchema, ValueType, WorkflowGraph,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{collections::BTreeMap, sync::Arc};

pub const KIND: &str = "builtin.dispatch";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}

#[derive(NodeInputs)]
struct DispatchInputs {
    selector: String,
    arguments: BTreeMap<String, ValueRef>,
    bindings: Option<BTreeMap<String, Inputs>>,
}

#[derive(Debug, Snafu)]
pub enum DispatchExecutionError {
    #[snafu(display("unknown dispatch branch `{branch}`"))]
    UnknownBranch { branch: String },
    #[snafu(display("dispatch argument `{parameter}` is already bound by the caller"))]
    BoundArgument { parameter: String },
    #[snafu(display("invalid arguments for dispatch branch `{branch}`: {source}"))]
    Arguments {
        branch: String,
        source: mf_runtime::WorkflowInputError,
    },
    #[snafu(display("dispatch branch `{branch}` failed: {source}"))]
    Call {
        branch: String,
        source: mf_runtime::WorkflowRunError,
    },
}

impl From<DispatchExecutionError> for NodeExecutionError {
    fn from(source: DispatchExecutionError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

struct Dispatch {
    id: String,
    branches: BTreeMap<String, PreparedSubgraph>,
}

impl NodePortContract for Dispatch {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: DispatchInputs::ports(),
            outputs: vec![PortSpec::new("result", ValueType::Object, true)],
        }
    }
}

impl TaskNode for Dispatch {
    fn supports_success_dependencies(&self) -> bool {
        false
    }

    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let DispatchInputs {
            selector,
            mut arguments,
            bindings,
        } = mf_runtime::decode_node_inputs(inputs)?;
        let bindings = bindings.unwrap_or_default();
        for branch in bindings.keys() {
            ensure!(
                self.branches.contains_key(branch),
                UnknownBranchSnafu { branch }
            );
        }
        let body = self
            .branches
            .get(&selector)
            .context(UnknownBranchSnafu { branch: &selector })?;
        if let Some(bound) = bindings.get(&selector) {
            for (name, value) in bound {
                ensure!(
                    !arguments.contains_key(name),
                    BoundArgumentSnafu { parameter: name }
                );
                arguments.insert(name.clone(), value.clone());
            }
        }
        let arguments = body
            .bind_arguments(arguments)
            .context(ArgumentsSnafu { branch: &selector })?;
        let outputs = context
            .run_call(&self.id, arguments, &body.nodes, |state| {
                body.execute_in_context(state)
            })
            .context(CallSnafu { branch: &selector })?;
        Ok(Outputs::from([(
            "result".into(),
            ValueRef::object(
                outputs
                    .into_iter()
                    .map(|(name, value)| (name.into(), value)),
            ),
        )])
        .into())
    }
}

fn factory(
    id: &str,
    config: Value,
    _: Value,
    mut parameters: PreparedSubgraphs,
) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let _: Config = mf_runtime::deserialize_config(config)?;
    let branches = mf_runtime::take_subgraph_collection(&mut parameters, "branches")?;
    ensure!(
        parameters.is_empty(),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "unexpected dispatch subgraph parameter"
        }
    );
    let mut graph = WorkflowGraph::default();
    for body in branches.values() {
        if let Some(body) = &body.graph {
            graph.nodes.extend(body.nodes.clone());
        }
    }
    let mut input_schemas = DispatchInputs::input_schemas();
    if !branches.is_empty() {
        input_schemas.insert(
            "selector".into(),
            ValueSchema::Enum(branches.keys().cloned().collect()),
        );
    }
    let mut input_bindings = BTreeMap::new();
    if branches
        .values()
        .any(|body| body.public_inputs().values().any(|input| input.secret))
    {
        for name in ["arguments", "bindings"] {
            input_bindings.insert(
                name.into(),
                mf_runtime::InputBinding {
                    secret: true,
                    ..Default::default()
                },
            );
        }
    }
    mf_runtime::PreparedNode::new(
        Dispatch {
            id: id.into(),
            branches,
        },
        NodeMetadata {
            input_schemas,
            input_bindings,
            subgraph: Some(Arc::new(graph)),
            ..Default::default()
        },
    )
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Subgraphs(factory) }
}
