use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled, instantiate_stream,
};
use mf_runtime::{
    ContextReference, ContextValue, ExecutionContext, Inputs, NodeBuildError, NodeExecutionError,
    NodeFactory, NodeInputs, NodeMetadata, NodeOutputs, NodePorts, NodeRegistration, NodeResult,
    Outputs, PortSpec, PreparedNode, StreamOptions, TaskNode, TypedNodeResult, TypedStreamNode,
    TypedTaskNode, ValueType, WorkflowArguments,
};
use serde_json::{Value, json};
use std::cell::Cell;

#[derive(NodeInputs)]
struct AddInputs {
    item: i64,
    label: Option<String>,
}

#[derive(NodeOutputs)]
struct AddOutputs {
    value: i64,
}

struct Add;
impl TypedTaskNode for Add {
    type Input = AddInputs;
    type Output = AddOutputs;

    fn execute(
        &self,
        input: AddInputs,
        ctx: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        let ContextValue::Value(source) = ctx.output("source.item")? else {
            panic!("source was skipped")
        };
        assert_eq!(source.as_i64(), Some(input.item));
        assert!(input.label.is_none());
        Ok(AddOutputs {
            value: input.item + 1,
        }
        .into())
    }
}

fn add(_: Value) -> Result<PreparedNode, NodeBuildError> {
    PreparedNode::typed_task(
        Add,
        NodeMetadata {
            context_references: vec![ContextReference::new("source.item", "source")],
            ..NodeMetadata::default()
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
#[derive(NodeInputs)]
struct ProducerInputs {
    start: Option<i64>,
}

#[derive(NodeOutputs)]
struct ProducerOutputs {
    item: i64,
    #[output(rename = "metric./~")]
    metric: f64,
}

struct TypedSource {
    invalid: bool,
    emitted: Cell<usize>,
}

impl TypedStreamNode for TypedSource {
    type Input = ProducerInputs;
    type Output = ProducerOutputs;

    fn execute(
        &mut self,
        input: ProducerInputs,
        _: &mut ExecutionContext,
        emit: &mut dyn FnMut(TypedNodeResult<ProducerOutputs>) -> Result<(), NodeExecutionError>,
    ) -> Result<(), NodeExecutionError> {
        let start = input.start.unwrap_or(1);
        for item in start..start + 3 {
            emit(
                ProducerOutputs {
                    item,
                    metric: if self.invalid { f64::NAN } else { 1.0 },
                }
                .into(),
            )?;
            self.emitted.set(self.emitted.get() + 1);
        }
        Ok(())
    }
}

fn source(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let ports = NodePorts {
        inputs: vec![],
        outputs: vec![PortSpec::new("item", ValueType::Int64, true)],
    };
    if config["stream"] == true {
        PreparedNode::typed_stream(
            TypedSource {
                invalid: config["invalid"] == true,
                emitted: Cell::new(0),
            },
            NodeMetadata::default(),
        )
    } else {
        Ok(PreparedNode::from_parts(
            mf_runtime::NodeExecution::Task(Box::new(Source)),
            ports,
        ))
    }
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
                .start_with_options(StreamOptions {
                    arguments: WorkflowArguments::from_json(br#"{"source":{"start":10}}"#).unwrap(),
                    ..Default::default()
                })
                .unwrap();
            for expected in 11..=13 {
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

fn string_sink(_: Value) -> Result<PreparedNode, NodeBuildError> {
    Ok(PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(Source)),
        NodePorts {
            inputs: vec![PortSpec::new("value", ValueType::String, true)],
            outputs: vec![PortSpec::new("item", ValueType::Int64, true)],
        },
    ))
}

inventory::submit! { NodeRegistration { kind: "test.typed_output_sink", factory: NodeFactory::Plain(string_sink) } }

#[test]
fn derived_outputs_reject_disjoint_consumers_before_execution() {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-03", "dependencies":{},
        "nodes":[
            {"id":"source", "kind":"test.typed_task_source"},
            {"id":"add", "kind":"test.typed_add"},
            {"id":"sink", "kind":"test.typed_output_sink"}
        ],
        "edges":[
            {"from_node":"source", "from_output":"item", "to_node":"add", "to_input":"item"},
            {"from_node":"add", "from_output":"value", "to_node":"sink", "to_input":"value"}
        ], "outputs":[]
    }))
    .unwrap();
    let error =
        compile_definition(&definition, &NodeRegistry::from_inventory().unwrap()).unwrap_err();
    assert!(
        matches!(error, mf_compiler::WorkflowCompileError::IncompatiblePortTypes { output_type, input_type, .. }
        if *output_type == ValueType::Int64 && *input_type == ValueType::String)
    );
}

#[test]
fn typed_producer_rejects_invalid_unobserved_outputs_before_publication() {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-03", "dependencies":{}, "execution":{"mode":"stream"},
        "nodes":[{"id":"source", "kind":"test.typed_task_source", "config":{"stream":true, "invalid":true}}],
        "outputs":[{"name":"result", "node":"source", "port":"item"}]
    })).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let compiled = compile_definition(&definition, &registry).unwrap();
    let instance = instantiate_stream(&compiled, &registry)
        .unwrap()
        .start()
        .unwrap();
    let error = instance.recv().unwrap_err();
    let mf_runtime::StreamError::Producer {
        definition_id,
        source,
    } = error
    else {
        panic!("expected an attributed producer failure");
    };
    assert_eq!(definition_id.as_str(), "source");
    let NodeExecutionError::OutputEncode { source } = source.as_ref() else {
        panic!("expected a typed encoding cause");
    };
    assert_eq!(source.pointer(), "/metric.~1~0");
    assert!(instance.join().is_err());
}
