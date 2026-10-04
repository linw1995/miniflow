use mf_compiler::{
    ExecutionContext, ExecutionDependency, FlowNode, Inputs, NodeExecutionError, NodePorts,
    OutputDerivation, Outputs, PortSpec, TaskNode, TypeInferenceState, ValueRef, ValueType,
    execute_node_in_context,
};
use serde_json::json;

struct Evidence {
    value: Option<ValueRef>,
    derivation: Option<OutputDerivation>,
}

impl TaskNode for Evidence {
    fn execute(
        &self,
        inputs: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok(Outputs::from([(
            "value".into(),
            self.value
                .clone()
                .unwrap_or_else(|| ValueRef::array([inputs["input"].clone()])),
        )])
        .into())
    }
}

fn node(
    id: &str,
    input: Option<ValueType>,
    output: ValueType,
    evidence: Evidence,
) -> mf_runtime::TaskFlowNode {
    let metadata = mf_runtime::NodeMetadata {
        output_derivations: evidence.derivation.iter().cloned().collect(),
        ..mf_runtime::NodeMetadata::new(NodePorts {
            inputs: input
                .map(|ty| PortSpec::new("input", ty, true))
                .into_iter()
                .collect(),
            outputs: vec![PortSpec::new("value", output, true)],
        })
    };
    FlowNode::new(id, mf_runtime::PreparedNode::new(evidence, metadata))
        .into_task()
        .unwrap()
}

fn dependency(source_node: &str) -> ExecutionDependency<'_> {
    ExecutionDependency {
        source_node,
        source_output: "value",
        input: Some("input"),
    }
}

#[test]
fn collection_inference_wraps_types_and_drops_exact_value_evidence() {
    for input_type in [
        ValueType::Int64,
        ValueType::Any,
        ValueType::List(Box::new(ValueType::String)),
    ] {
        let mut inference = TypeInferenceState::default();
        let mut source = node(
            "source",
            None,
            input_type.clone(),
            Evidence {
                value: None,
                derivation: None,
            },
        );
        inference.resolve_node(&mut source, &[]).unwrap();
        let mut collect = node(
            "collect",
            Some(ValueType::Any),
            ValueType::Array,
            Evidence {
                value: None,
                derivation: Some(OutputDerivation::collect_input("value", "input")),
            },
        );
        inference
            .resolve_node(&mut collect, &[dependency("source")])
            .unwrap();
        assert_eq!(
            collect.metadata.ports.outputs[0].value_type,
            ValueType::List(Box::new(input_type))
        );
    }

    let value: ValueRef = json!([1, "invalid"]).into();
    let mut inference = TypeInferenceState::default();
    let mut source = node(
        "source",
        None,
        ValueType::Any,
        Evidence {
            value: Some(value.clone()),
            derivation: Some(OutputDerivation::literal("value", value)),
        },
    );
    let mut collect = node(
        "collect",
        Some(ValueType::Any),
        ValueType::Array,
        Evidence {
            value: None,
            derivation: Some(OutputDerivation::collect_input("value", "input")),
        },
    );
    let mut sink = node(
        "sink",
        Some(ValueType::List(Box::new(ValueType::List(Box::new(
            ValueType::Int64,
        ))))),
        ValueType::Any,
        Evidence {
            value: None,
            derivation: None,
        },
    );
    inference.resolve_node(&mut source, &[]).unwrap();
    inference
        .resolve_node(&mut collect, &[dependency("source")])
        .unwrap();
    inference
        .resolve_node(&mut sink, &[dependency("collect")])
        .unwrap();
    let mut context = ExecutionContext::default();
    execute_node_in_context(&source, &[], &mut context).unwrap();
    execute_node_in_context(&collect, &[dependency("source")], &mut context).unwrap();
    let error = execute_node_in_context(&sink, &[dependency("collect")], &mut context)
        .unwrap_err()
        .to_string();
    assert!(error.contains("/0/1"), "{error}");
}

#[test]
fn collection_metadata_rejects_invalid_ports_types_and_excessive_depth() {
    for (input, output, derivation) in [
        (
            ValueType::Any,
            ValueType::Array,
            OutputDerivation::collect_input("value", "missing"),
        ),
        (
            ValueType::Any,
            ValueType::Array,
            OutputDerivation::collect_input("missing", "input"),
        ),
        (
            ValueType::Int64,
            ValueType::String,
            OutputDerivation::collect_input("value", "input"),
        ),
        (
            (1..ValueType::MAX_DEPTH).fold(ValueType::Int64, |ty, _| ValueType::List(Box::new(ty))),
            ValueType::Array,
            OutputDerivation::collect_input("value", "input"),
        ),
    ] {
        let mut collect = node(
            "collect",
            Some(input),
            output,
            Evidence {
                value: None,
                derivation: Some(derivation),
            },
        );
        let error = TypeInferenceState::default()
            .resolve_node(&mut collect, &[])
            .unwrap_err()
            .to_string();
        assert!(error.contains("collect"), "{error}");
    }
}
