use mf_runtime::{
    EmissionSink, ExecutionContext, Inputs, NodeExecution, NodeMetadata, NodePortContract,
    NodePorts, NodeResult, Outputs, PortSpec, PreparedNode, PreparedSubgraph, StreamError,
    SubgraphNode, SubgraphStream, ValueType,
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
        let mut stream = SubgraphStream::new("invoke", context, sink);
        Ok(self.body.execute_with_sink(context, &mut stream)?.into())
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

    let mut sink = Collect::default();
    let result = task
        .execute_with_sink(Inputs::new(), &mut ExecutionContext::default(), &mut sink)
        .unwrap();
    assert_eq!(result.outputs, output());
    assert!(sink.emissions.is_empty());
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

#[test]
fn public_subgraph_stream_fans_in_concurrent_producers_on_the_caller_thread() {
    struct ThreadCollect {
        caller: std::thread::ThreadId,
        values: Vec<i64>,
    }
    impl EmissionSink for ThreadCollect {
        fn send(&mut self, result: NodeResult) -> Result<(), StreamError> {
            assert_eq!(std::thread::current().id(), self.caller);
            self.values.push(result.outputs["delta"].as_i64().unwrap());
            Ok(())
        }
    }

    let context = ExecutionContext::default();
    let mut sink = ThreadCollect {
        caller: std::thread::current().id(),
        values: Vec::new(),
    };
    let mut stream = SubgraphStream::new("invoke", &context, &mut sink);
    let completed = stream
        .forward(|sender| {
            std::thread::scope(|scope| {
                for value in [1, 2] {
                    let mut sender = sender.clone();
                    scope.spawn(move || {
                        sender
                            .send(Outputs::from([("delta".into(), value.into())]).into())
                            .unwrap();
                    });
                }
            });
            Ok("completed")
        })
        .unwrap();
    assert_eq!(completed, "completed");
    sink.values.sort_unstable();
    assert_eq!(sink.values, [1, 2]);
}

#[test]
fn public_subgraph_stream_cancels_on_direct_and_forwarded_delivery_failures() {
    struct Reject;
    impl EmissionSink for Reject {
        fn send(&mut self, _: NodeResult) -> Result<(), StreamError> {
            mf_runtime::StreamExecutionSnafu {
                message: "delivery sentinel",
            }
            .fail()
        }
    }

    for forwarded in [false, true] {
        let context = ExecutionContext::default();
        let cancellation = context.cancellation();
        let mut reject = Reject;
        let mut stream = SubgraphStream::new("invoke", &context, &mut reject);
        let result = if forwarded {
            stream.forward(|mut sender| sender.send(output().into()).map_err(Into::into))
        } else {
            stream.send(output().into()).map_err(Into::into)
        };
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("delivery sentinel")
        );
        assert!(
            cancellation
                .failure()
                .unwrap()
                .to_string()
                .contains("delivery sentinel")
        );
    }
}
