use mf_runtime::{
    ExecutionContext, ExecutionScope, Inputs, LoopBodyDefinition, Node, NodeBuildError,
    NodeDefinition, NodeExecutionError, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec,
    PreparedSubgraph, SubgraphDefinition, ValueType, WorkflowOutputDefinition,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Config {
    body: LoopBodyDefinition,
    result: mf_runtime::IterationResultDefinition,
}

struct Declaration(Config);

impl Node for Declaration {
    fn subgraph_definition(
        &self,
        _: &NodeDefinition,
    ) -> Result<Option<SubgraphDefinition>, NodeBuildError> {
        Ok(Some(SubgraphDefinition {
            body: self.0.body.clone(),
            body_pointer: "/config/body".into(),
            source_id: "@custom".into(),
            inputs: BTreeMap::from([("input".into(), ValueType::Number)]),
            outputs: vec![WorkflowOutputDefinition {
                name: "result".into(),
                node: self.0.result.node.clone(),
                port: self.0.result.port.clone(),
                optional: false,
            }],
            options: Value::Null,
            allow_state: false,
        }))
    }

    fn with_subgraph(
        self: Box<Self>,
        id: &str,
        _: Value,
        body: PreparedSubgraph,
    ) -> Result<Box<dyn Node>, NodeBuildError> {
        Ok(Box::new(Executor {
            id: id.into(),
            body,
        }))
    }

    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(NodeExecutionError::ExecutionFailed {
            message: "body was not bound".into(),
        })
    }
}

struct Executor {
    id: String,
    body: PreparedSubgraph,
}

impl Node for Executor {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(NodeExecutionError::ExecutionFailed {
            message: "execution context is required".into(),
        })
    }

    fn ports(&self) -> Option<NodePorts> {
        Some(NodePorts {
            inputs: vec![PortSpec::new("input", ValueType::Number, true)],
            outputs: vec![PortSpec::new("value", ValueType::Number, true)],
        })
    }

    fn execute_with_context_mut(
        &self,
        inputs: Inputs,
        state: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let mut total = 0;
        for index in 0..2 {
            let scope = ExecutionScope::new(
                &self.id,
                "@custom",
                index,
                inputs.clone(),
                BTreeMap::from([("input".into(), ValueType::Number)]),
            )?;
            let (outputs, _, _) = state
                .run_scope(scope, |state| self.body.execute_in_context(state))
                .map_err(|source| NodeExecutionError::PluginFailed {
                    source: Box::new(source),
                })?;
            total += outputs["result"].as_i64().unwrap();
        }
        Ok(Outputs::from([("value".into(), Value::from(total))]).into())
    }
}

fn factory(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    Ok(Box::new(Declaration(mf_runtime::deserialize_config(
        config,
    )?)))
}

inventory::submit! {
    NodeRegistration { kind: "fixture.subgraph", inputs: &[], outputs: &[], factory }
}
