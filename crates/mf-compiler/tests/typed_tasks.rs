use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled, instantiate_stream,
};
use mf_runtime::{
    ContextReference, ContextValue, Emitter, ExecutionContext, Inputs, NodeBuildError,
    NodeExecutionError, NodeFactory, NodeInputs, NodeMetadata, NodePorts, NodeRegistration,
    NodeResult, Outputs, PortSpec, PreparedNode, StreamNode, TaskNode, TypedTaskNode, ValueType,
};
use serde_json::{Value, json};

#[derive(NodeInputs)]
struct AddInputs {
    item: i64,
    label: Option<String>,
}

struct Add;
impl TypedTaskNode for Add {
    type Input = AddInputs;

    fn execute(
        &self,
        input: AddInputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let ContextValue::Value(source) = ctx.output("source.item")? else {
            panic!("source was skipped")
        };
        assert_eq!(source.as_i64(), Some(input.item));
        assert!(input.label.is_none());
        Ok(Outputs::from([("value".into(), (input.item + 1).into())]).into())
    }
}

fn add(_: Value) -> Result<PreparedNode, NodeBuildError> {
    PreparedNode::typed_task(
        Add,
        NodeMetadata {
            context_references: vec![ContextReference::new("source.item", "source")],
            ..NodeMetadata::new(NodePorts {
                inputs: vec![],
                outputs: vec![PortSpec::new("value", ValueType::Int64, true)],
            })
        },
    )
}

struct Source;
impl TaskNode for Source {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        Ok(Outputs::from([("item".into(), 3.into())]).into())
    }
}
impl StreamNode for Source {
    fn execute(
        &mut self,
        _: Inputs,
        _: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        for item in 1..=3 {
            emitter.send(Outputs::from([("item".into(), item.into())]).into())?;
        }
        Ok(())
    }
}

fn source(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let ports = NodePorts {
        inputs: vec![],
        outputs: vec![PortSpec::new("item", ValueType::Int64, true)],
    };
    Ok(if config["stream"] == true {
        PreparedNode::stream(Source, ports)
    } else {
        PreparedNode::new(Source, ports)
    })
}

inventory::submit! { NodeRegistration { kind: "test.typed_add", factory: NodeFactory::Plain(add) } }
inventory::submit! { NodeRegistration { kind: "test.typed_task_source", factory: NodeFactory::Plain(source) } }

#[test]
fn typed_tasks_use_the_same_adapter_and_context_in_task_and_stream_domains() {
    for streaming in [false, true] {
        let mut value = json!({
            "version":"2026-10-03", "dependencies":{},
            "nodes":[
                {"id":"source", "kind":"test.typed_task_source", "config":{"stream":streaming}},
                {"id":"add", "kind":"test.typed_add"}
            ],
            "edges":[{"from_node":"source", "from_output":"item", "to_node":"add", "to_input":"item"}],
            "outputs":[{"name":"result", "node":"add", "port":"value"}]
        });
        if streaming {
            value["execution"] =
                json!({"mode":"stream", "limits":{"max_pending_messages":2,"workers":1}});
        }
        let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
        let registry = NodeRegistry::from_inventory().unwrap();
        let compiled = compile_definition(&definition, &registry).unwrap();
        if streaming {
            let instance = instantiate_stream(&compiled, &registry)
                .unwrap()
                .start()
                .unwrap();
            for expected in 2..=4 {
                assert_eq!(
                    instance.recv().unwrap().unwrap().outputs["result"],
                    json!(expected)
                );
            }
            assert!(instance.recv().unwrap().is_none());
            assert_eq!(instance.join().unwrap().delivered_outputs, 3);
        } else {
            let flow = instantiate_compiled(&compiled, &registry).unwrap();
            assert_eq!(flow.execute().unwrap()["result"], json!(4));
        }
    }
}
