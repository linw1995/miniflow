use mf_runtime::{
    ExecutionContext, NodePorts, NodeValue, NodeValues, RustValueType, TypedConstructor,
    TypedNodeResult, TypedNodeValue, TypedTaskHandle, TypedTaskNode,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(NodeValue)]
#[value(typed)]
struct Text {
    text: String,
}

struct Stateful(Arc<AtomicUsize>);
impl TypedTaskNode for Stateful {
    type Input = Text;
    type Output = Text;
    fn execute(
        &self,
        input: Text,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Text>, mf_runtime::NodeExecutionError> {
        let count = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(Text {
            text: format!("{}:{count}", input.text),
        }
        .into())
    }
}

#[test]
fn typed_and_dynamic_handles_share_one_private_executor() {
    let count = Arc::new(AtomicUsize::new(0));
    let handle = TypedTaskHandle::new(
        Stateful(count.clone()),
        NodePorts::default(),
        TypedConstructor {
            package: "fixture",
            path: &["text"],
        },
        true,
    )
    .unwrap();
    let result = handle
        .clone()
        .execute(
            handle.input_from_fields(("hello".into(),)),
            &mut ExecutionContext::default(),
        )
        .unwrap();
    assert_eq!(result.outputs.into_fields(), ("hello:1".to_owned(),));
    let prepared = handle.prepared();
    let result = prepared
        .execution
        .as_task_node()
        .unwrap()
        .execute(
            NodeValues::from([("text".into(), "hello".into())]),
            &mut ExecutionContext::default(),
        )
        .unwrap();
    assert_eq!(result.outputs["text"].as_str(), Some("hello:2"));
    assert_eq!(count.load(Ordering::SeqCst), 2);
    let generation = prepared.metadata.typed_generation.unwrap();
    assert_eq!(generation.inputs[0].rust_type, RustValueType::String);
    generation.validate(&prepared.metadata.ports).unwrap();
    assert!(
        generation
            .constructor
            .dependency_alias(&Default::default())
            .is_err()
    );
    let dependencies =
        serde_json::from_value(serde_json::json!({ "alias": {"package": "fixture"} })).unwrap();
    assert_eq!(
        generation
            .constructor
            .dependency_alias(&dependencies)
            .unwrap(),
        "alias"
    );
    let mut invalid = generation.clone();
    invalid.inputs[0].port.name = "other".into();
    assert!(invalid.validate(&prepared.metadata.ports).is_err());
    invalid = generation;
    invalid.constructor.path = &["bad/path"];
    assert!(invalid.validate(&prepared.metadata.ports).is_err());
}

#[derive(NodeValue)]
#[value(typed)]
struct Floats {
    #[value(rename = "rows./~")]
    rows: Vec<std::collections::BTreeMap<String, f64>>,
    label: Option<String>,
}

#[test]
fn borrowed_validation_matches_encoding_without_consuming_fields() {
    let invalid = Floats {
        rows: vec![std::collections::BTreeMap::from([("a/b".into(), f64::NAN)])],
        label: None,
    };
    let validation = invalid.validate_typed().unwrap_err();
    let encoding = invalid.into_values().unwrap_err();
    assert_eq!(validation.pointer(), "/rows.~1~0/0/a~1b");
    assert_eq!(validation.to_string(), encoding.to_string());
    let valid = Floats {
        rows: vec![],
        label: None,
    };
    assert_eq!(valid.validate_typed().unwrap(), ["rows./~"]);
    assert!(valid.into_fields().1.is_none());
}
