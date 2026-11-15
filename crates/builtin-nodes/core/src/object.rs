use mf_runtime::{
    ExecutionContext, FlowBindingSource, FlowBindings, Inputs, NodeBuildError, NodeExecutionError,
    NodeFactory, NodeMetadata, NodePortContract, NodePorts, NodeRegistration, NodeResult,
    OutputDerivation, Outputs, PortSpec, PreparedNode, TaskNode, ValueRef, ValueType, decode_input,
    deserialize_config, reject_unknown_inputs,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{ResultExt, Snafu, ensure};
use std::collections::BTreeMap;

pub const KIND: &str = "builtin.object";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    fields: BTreeMap<String, ValueType>,
}

#[derive(Debug, Snafu)]
pub enum ObjectBuildError {
    #[snafu(display("object field names must be nonblank"))]
    BlankField,
    #[snafu(display("object output type is too deeply nested: {source}"))]
    OutputTypeDepth { source: mf_runtime::TypeDepthError },
}

impl From<ObjectBuildError> for NodeBuildError {
    fn from(source: ObjectBuildError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

#[derive(Debug, Snafu)]
pub enum ObjectExecutionError {
    #[snafu(transparent)]
    Input {
        source: mf_runtime::InputDecodeError,
    },
    #[snafu(display("object field `{field}`: {source}"))]
    FieldType {
        field: String,
        source: mf_runtime::TypeMismatch,
    },
}

impl From<ObjectExecutionError> for NodeExecutionError {
    fn from(source: ObjectExecutionError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

struct ObjectNode {
    fields: BTreeMap<String, ValueType>,
    output_type: ValueType,
}

impl NodePortContract for ObjectNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: self
                .fields
                .iter()
                .map(|(name, value_type)| PortSpec::owned(name, value_type.clone(), true))
                .collect(),
            outputs: vec![PortSpec::new("value", self.output_type.clone(), true)],
        }
    }
}

impl TaskNode for ObjectNode {
    fn execute(
        &self,
        mut inputs: Inputs,
        _context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let mut entries = Vec::with_capacity(self.fields.len());
        for (field, value_type) in &self.fields {
            let value: ValueRef =
                decode_input(&mut inputs, field).map_err(ObjectExecutionError::from)?;
            value_type
                .validate_shared(&value)
                .context(FieldTypeSnafu { field })?;
            entries.push((field.as_str().into(), value));
        }
        reject_unknown_inputs(inputs).map_err(ObjectExecutionError::from)?;
        // Preserve each child's payload and privacy independently. A private child
        // does not make the container or its unrelated siblings private.
        Ok(Outputs::from([("value".into(), ValueRef::object(entries))]).into())
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let Config { fields } = deserialize_config(config)?;
    for field in fields.keys() {
        ensure!(!field.trim().is_empty(), BlankFieldSnafu);
    }
    let mut types = fields.values();
    let common_type = match types.next() {
        Some(first) if types.all(|other| other == first) => first.clone(),
        _ => ValueType::Any,
    };
    let output_type = ValueType::Map(Box::new(common_type));
    output_type.check_depth().context(OutputTypeDepthSnafu)?;
    let metadata = NodeMetadata {
        // Native values are capabilities, not serializable literal evidence.
        output_derivations: vec![OutputDerivation::known_type("value", output_type.clone())],
        ..Default::default()
    };
    PreparedNode::new(
        ObjectNode {
            fields,
            output_type,
        },
        metadata,
    )
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        factory: NodeFactory::WithFlowBindings {
            factory: &NodeFactory::Plain(factory),
            bindings: |config| FlowBindings {
                outputs: BTreeMap::from([("value".into(), FlowBindingSource::Object(
                    config.get("fields").and_then(Value::as_object).into_iter().flatten()
                        .map(|(field, _)| (field.clone(), field.clone())).collect(),
                ))]),
                ..Default::default()
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{FlowValue, PreparedSubgraph};
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn prepare(fields: Value) -> PreparedNode {
        mf_runtime::NodeRegistry::from_inventory()
            .unwrap()
            .get(KIND)
            .unwrap()
            .instantiate(json!({"fields": fields}))
            .unwrap()
    }

    fn execute(prepared: &PreparedNode, inputs: Inputs) -> ValueRef {
        prepared
            .execution
            .as_task_node()
            .unwrap()
            .execute(inputs, &mut ExecutionContext::default())
            .unwrap()
            .outputs
            .remove("value")
            .unwrap()
    }

    fn map(inner: ValueType) -> ValueType {
        ValueType::Map(Box::new(inner))
    }

    fn execution_error(error: &NodeExecutionError) -> &ObjectExecutionError {
        let NodeExecutionError::PluginFailed { source } = error else {
            panic!("expected an object error, got {error}");
        };
        source.downcast_ref().unwrap()
    }

    #[test]
    fn declaration_is_strict_but_business_keys_are_not_identifiers_or_reserved() {
        for config in [
            json!({}),
            json!({"fields": null}),
            json!({"fields": []}),
            json!({"fields": {}, "extra": true}),
            json!({"fields": {"": "any"}}),
            json!({"fields": {" \t\n": "any"}}),
            json!({"fields": {"x": "unknown"}}),
            json!({"fields": {"x": {"map": "int", "extra": true}}}),
        ] {
            assert!(factory(config.clone()).is_err(), "accepted {config}");
        }
        let error = factory(json!({"fields": {" ": "any"}})).err().unwrap();
        let NodeBuildError::FactoryFailed { source } = error else {
            panic!("expected an owning-node build error");
        };
        assert!(matches!(
            source.downcast_ref::<ObjectBuildError>(),
            Some(ObjectBuildError::BlankField)
        ));

        let prepared = prepare(json!({
            "value": "int", "a.b/c~d": "int", "two words": "int", "工具-1": "int"
        }));
        let inputs: Inputs = ["value", "a.b/c~d", "two words", "工具-1"]
            .into_iter()
            .map(|name| (name.into(), ValueRef::from(42)))
            .collect();
        let output = execute(&prepared, inputs.clone());
        for (name, value) in inputs {
            assert!(output[name.as_str()].ptr_eq(&value));
        }
        assert_eq!(output.as_object().unwrap().len(), 4);
    }

    #[test]
    fn common_types_and_required_ports_are_reflected_without_literal_evidence() {
        for (fields, common) in [
            (json!({}), ValueType::Any),
            (json!({"a": "flow", "b": "flow"}), ValueType::Flow),
            (json!({"a": "int", "b": "int"}), ValueType::Int64),
            (json!({"a": "int", "b": "number"}), ValueType::Any),
            (json!({"a": "any", "b": "flow"}), ValueType::Any),
            (
                json!({"a": {"list": {"nullable": "flow"}}, "b": {"list": {"nullable": "flow"}}}),
                ValueType::List(Box::new(ValueType::Nullable(Box::new(ValueType::Flow)))),
            ),
        ] {
            let expected_fields: BTreeMap<String, ValueType> =
                serde_json::from_value(fields.clone()).unwrap();
            let prepared = prepare(fields);
            assert_eq!(
                prepared.metadata.ports.inputs,
                expected_fields
                    .into_iter()
                    .map(|(name, ty)| PortSpec::owned(name, ty, true))
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                prepared.metadata.ports.outputs,
                [PortSpec::new("value", map(common.clone()), true)]
            );
            assert_eq!(
                prepared.metadata.output_derivations,
                [OutputDerivation::known_type("value", map(common))]
            );
            prepared
                .metadata
                .ports
                .validate_derivations("object", &prepared.metadata.output_derivations)
                .unwrap();
            serde_json::to_value(&prepared.metadata).unwrap();
            assert!(prepared.metadata.input_bindings.is_empty());
            assert!(prepared.metadata.context_references.is_empty());
        }
    }

    #[test]
    fn output_container_respects_the_type_depth_limit() {
        let mut ty = ValueType::Flow;
        for _ in 1..ValueType::MAX_DEPTH {
            ty = ValueType::List(Box::new(ty));
        }
        ty.check_depth().unwrap();
        let error = factory(json!({"fields": {"nested": ty}})).err().unwrap();
        let NodeBuildError::FactoryFailed { source } = error else {
            panic!("expected typed output depth error");
        };
        assert!(matches!(
            source.downcast_ref::<ObjectBuildError>(),
            Some(ObjectBuildError::OutputTypeDepth { .. })
        ));
    }

    #[test]
    fn json_fields_keep_payload_identity_and_independent_secret_flags() {
        let prepared = prepare(json!({
            "count": "int", "private": "string", "nested": "object", "nothing": "null"
        }));
        let inputs = Inputs::from([
            ("count".into(), ValueRef::from(7)),
            ("private".into(), ValueRef::from("hidden").into_secret()),
            (
                "nested".into(),
                ValueRef::object([
                    ("public".into(), true.into()),
                    (
                        "private".into(),
                        ValueRef::from("nested secret").into_secret(),
                    ),
                ]),
            ),
            ("nothing".into(), ValueRef::null()),
        ]);
        let output = execute(&prepared, inputs.clone());
        assert!(!output.is_secret());
        assert!(!output["count"].is_secret());
        assert!(output["private"].is_secret());
        assert!(!output["nested"].is_secret());
        assert!(output["nested"]["private"].is_secret());
        for (name, value) in inputs {
            assert!(output[name.as_str()].ptr_eq(&value));
        }
        assert_eq!(
            serde_json::to_value(output.redacted()).unwrap(),
            json!({"count": 7, "private": "[REDACTED]", "nested": {
                "public": true, "private": "[REDACTED]"
            }, "nothing": null})
        );
    }

    #[test]
    fn native_flows_and_recursive_containers_are_never_serialized_or_invoked() {
        let executions = Arc::new(AtomicUsize::new(0));
        let effects = executions.clone();
        let flow: ValueRef =
            FlowValue::new(PreparedSubgraph::new(Vec::new(), Vec::new(), move |_| {
                effects.fetch_add(1, Ordering::SeqCst);
                Ok(Outputs::new())
            }))
            .unwrap()
            .into();
        let direct = prepare(json!({"first": "flow", "second": "flow"}));
        let output = execute(
            &direct,
            Inputs::from([
                ("first".into(), flow.clone()),
                ("second".into(), flow.clone().into_secret()),
            ]),
        );
        assert!(output["first"].ptr_eq(&flow));
        assert_eq!(output["second"].as_flow(), flow.as_flow());
        assert!(output["second"].is_secret());
        assert!(!output.is_secret());

        let nested = ValueRef::array([
            ValueRef::object([("native".into(), flow.clone())]),
            ValueRef::object([("absent".into(), ValueRef::null())]),
        ]);
        let recursive = prepare(json!({"nested": {"list": {"map": {"nullable": "flow"}}}}));
        let output = execute(
            &recursive,
            Inputs::from([("nested".into(), nested.clone())]),
        );
        assert!(output["nested"].ptr_eq(&nested));
        assert!(output["nested"][0]["native"].ptr_eq(&flow));
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        output["nested"][0]["native"]
            .as_flow()
            .unwrap()
            .invoke("call", Default::default(), &mut ExecutionContext::default())
            .unwrap();
        assert_eq!(executions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn direct_execution_rejects_missing_extra_and_wrongly_typed_inputs() {
        let prepared = prepare(json!({"x": {"list": {"map": "int"}}}));
        let task = prepared.execution.as_task_node().unwrap();
        for (inputs, expected) in [
            (Inputs::new(), "missing"),
            (
                Inputs::from([
                    ("x".into(), json!([{"count": 1}]).into()),
                    ("extra".into(), true.into()),
                ]),
                "extra",
            ),
            (
                Inputs::from([("x".into(), json!([{"count": "wrong"}]).into())]),
                "type",
            ),
        ] {
            let error = task
                .execute(inputs, &mut ExecutionContext::default())
                .unwrap_err();
            match (execution_error(&error), expected) {
                (
                    ObjectExecutionError::Input {
                        source: mf_runtime::InputDecodeError::MissingField { port },
                    },
                    "missing",
                ) => assert_eq!(port, "x"),
                (
                    ObjectExecutionError::Input {
                        source: mf_runtime::InputDecodeError::UnknownField { port },
                    },
                    "extra",
                ) => assert_eq!(port, "extra"),
                (ObjectExecutionError::FieldType { field, source }, "type") => {
                    assert_eq!(field, "x");
                    assert_eq!(source.path, "/0/count");
                    assert_eq!(source.expected, ValueType::Int64);
                }
                (error, expected) => panic!("expected {expected}, got {error}"),
            }
        }
        let empty = prepare(json!({}));
        assert!(
            empty
                .execution
                .as_task_node()
                .unwrap()
                .execute(
                    Inputs::from([("extra".into(), true.into())]),
                    &mut ExecutionContext::default()
                )
                .is_err()
        );
    }

    #[derive(mf_runtime::NodeValue)]
    struct FlowMapInput {
        flows: BTreeMap<String, FlowValue>,
    }

    #[derive(mf_runtime::NodeValue)]
    struct EmptyOutput {}

    struct EmptyFlowMapConsumer;

    impl mf_runtime::TypedTaskNode for EmptyFlowMapConsumer {
        type Input = FlowMapInput;
        type Output = EmptyOutput;

        fn execute(
            &self,
            input: FlowMapInput,
            _: &mut ExecutionContext,
        ) -> Result<mf_runtime::TypedNodeResult<EmptyOutput>, NodeExecutionError> {
            assert!(input.flows.is_empty());
            Ok(EmptyOutput {}.into())
        }
    }

    #[test]
    fn zero_fields_produce_an_empty_map_accepted_by_a_checked_flow_map_edge() {
        let prepared = prepare(json!({}));
        assert_eq!(execute(&prepared, Inputs::new()), json!({}));
        let consumer =
            PreparedNode::typed_task(EmptyFlowMapConsumer, NodeMetadata::default()).unwrap();
        assert_eq!(
            prepared.metadata.ports.outputs[0]
                .value_type
                .compatibility_with(&consumer.metadata.ports.inputs[0].value_type),
            mf_runtime::TypeCompatibility::Checked
        );
        let source = mf_runtime::FlowNode::new("object", prepared)
            .into_task()
            .unwrap();
        let target = mf_runtime::FlowNode::new("consumer", consumer)
            .into_task()
            .unwrap();
        let mut context = ExecutionContext::default();
        mf_runtime::execute_node_in_context(&source, &[], &mut context).unwrap();
        mf_runtime::execute_node_in_context(
            &target,
            &[mf_runtime::ExecutionDependency {
                exclusive: false,
                input: Some("flows"),
                source_node: "object",
                source_output: "value",
                source_key: "object.value".into(),
            }],
            &mut context,
        )
        .unwrap();
    }
}
