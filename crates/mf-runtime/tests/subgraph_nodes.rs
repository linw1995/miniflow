use mf_runtime::{
    EmissionSink, ExecutionContext, Inputs, NodeExecution, NodeMetadata, NodePortContract,
    NodePorts, NodeResult, Outputs, PortSpec, PreparedNode, PreparedSubgraph, StreamError,
    SubgraphNode, ValueType,
};
use serde_json::json;

struct Invoke {
    body: PreparedSubgraph,
}

impl NodePortContract for Invoke {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: Vec::new(),
            outputs: vec![PortSpec::new("value", ValueType::Int64, true)],
        }
    }
}

impl SubgraphNode for Invoke {
    fn body(&self) -> &PreparedSubgraph {
        &self.body
    }

    fn supports_success_dependencies(&self) -> bool {
        false
    }

    fn execute(
        &self,
        _inputs: Inputs,
        context: &mut ExecutionContext,
        sink: &mut dyn EmissionSink,
    ) -> Result<NodeResult, mf_runtime::NodeExecutionError> {
        Ok(self.body.execute_with_sink(context, sink)?.into())
    }
}

fn output() -> Outputs {
    Outputs::from([("value".into(), json!(42).into())])
}

#[test]
fn public_subgraph_contract_prepares_and_executes_terminal_nodes() {
    let body = PreparedSubgraph::new(Vec::new(), Vec::new(), |_| Ok(output()));
    let prepared = PreparedNode::subgraph(Invoke { body }, NodeMetadata::default()).unwrap();
    assert!(matches!(prepared.execution, NodeExecution::Task(_)));
    assert!(prepared.metadata.stream_outputs.is_empty());

    let task = prepared.execution.as_task_node().unwrap();
    assert!(!task.supports_success_dependencies());
    let result = task
        .execute(Inputs::new(), &mut ExecutionContext::default())
        .unwrap();
    assert_eq!(result.outputs, output());
}

#[derive(Default)]
struct Collect {
    emissions: Vec<NodeResult>,
}

impl EmissionSink for Collect {
    fn send(&mut self, result: NodeResult) -> Result<(), StreamError> {
        self.emissions.push(result);
        Ok(())
    }
}

#[test]
fn public_subgraph_contract_forwards_streams_through_the_same_execution_method() {
    let body = PreparedSubgraph::streaming(
        Vec::new(),
        Vec::new(),
        vec![PortSpec::new("delta", ValueType::Int64, true)],
        |_, sink| {
            sink.send(Outputs::from([("delta".into(), json!(1).into())]).into())
                .unwrap();
            Ok(output())
        },
    );
    let prepared = PreparedNode::subgraph(Invoke { body }, NodeMetadata::default()).unwrap();
    assert!(matches!(prepared.execution, NodeExecution::TaskStream(_)));
    assert_eq!(prepared.metadata.stream_outputs[0].name, "delta");

    let task = prepared.execution.as_task_node().unwrap();
    assert!(!task.supports_success_dependencies());
    let mut sink = Collect::default();
    let result = task
        .execute_with_sink(Inputs::new(), &mut ExecutionContext::default(), &mut sink)
        .unwrap();
    assert_eq!(result.outputs, output());
    assert_eq!(sink.emissions.len(), 1);
    assert_eq!(sink.emissions[0].outputs["delta"], json!(1));
}
