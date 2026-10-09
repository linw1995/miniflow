extern crate mf_runtime as runtime_alias;
use mf_runtime::{
    InputDecodeError, InputValue, NodeInputs, NodeOutputs, NodeValue, OutputEncodeError,
    OutputValue, Outputs, TypeDepthError, TypeMismatch, ValueRef, ValueType, decode_input,
    encode_output,
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error};

type Label = Option<String>;
type Rows<T> = Vec<BTreeMap<String, T>>;

#[derive(NodeValue)]
#[value(runtime = "::runtime_alias")]
struct Fields<T> {
    active: bool,
    count: i64,
    ratio: f64,
    r#type: String,
    #[value(rename = "rows./~")]
    rows: Rows<T>,
    shared: ValueRef,
    label: Label,
    raw: Option<ValueRef>,
}

#[test]
fn output_descriptors_and_encoded_values_round_trip_through_input_codecs() {
    type Contract = Fields<i64>;
    assert_eq!(
        <Contract as NodeOutputs>::ports(),
        <Contract as NodeInputs>::ports()
    );
    let ports = <Contract as NodeValue>::ports();
    assert_eq!(ports[3].name, "type");
    assert_eq!(ports[4].name, "rows./~");
    assert_eq!(
        ports[4].value_type,
        ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Int64))))
    );
    assert!(!ports[6].required && !ports[7].required);
    let shared = ValueRef::from(json!({"nested": [1, true]}));
    let outputs = Contract {
        active: true,
        count: i64::MIN,
        ratio: -0.0,
        r#type: "example".into(),
        rows: vec![BTreeMap::from([("count".into(), i64::MAX)])],
        shared: shared.clone(),
        label: Some("present".into()),
        raw: Some(ValueRef::null()),
    }
    .into_outputs()
    .unwrap();
    assert_eq!(outputs["label"], json!("present"));
    assert!(outputs["raw"].is_null());
    for port in ports {
        if let Some(value) = outputs.get(port.name.as_ref()) {
            port.value_type.validate_shared(value).unwrap();
        } else {
            assert!(!port.required);
        }
    }
    let decoded = Contract::from_values(outputs).unwrap();
    assert!(decoded.active);
    assert_eq!(decoded.count, i64::MIN);
    assert_eq!(decoded.ratio.to_bits(), (-0.0_f64).to_bits());
    assert_eq!(decoded.r#type, "example");
    assert_eq!(decoded.rows[0]["count"], i64::MAX);
    assert!(decoded.shared.ptr_eq(&shared));
    assert_eq!(decoded.label.as_deref(), Some("present"));
    assert!(decoded.raw.unwrap().is_null());
}

#[test]
fn collections_preserve_shared_descendants() {
    let shared = ValueRef::from(json!({"items":[{"nested":[1,2]},null]}));
    let mut outputs = Outputs::new();
    encode_output(&mut outputs, "root", shared.clone()).unwrap();
    encode_output(&mut outputs, "list", vec![shared["items"][0].clone()]).unwrap();
    encode_output(
        &mut outputs,
        "map",
        BTreeMap::from([("raw".into(), shared["items"][1].clone())]),
    )
    .unwrap();
    assert!(outputs["root"].ptr_eq(&shared));
    assert!(outputs["list"][0].ptr_eq(&shared["items"][0]));
    assert!(outputs["map"]["raw"].ptr_eq(&shared["items"][1]));
}

#[test]
fn non_finite_numbers_retain_nested_paths_and_typed_sources() {
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let error = encode_output(
            &mut Outputs::new(),
            "rows./~",
            vec![BTreeMap::from([("x/y~z".into(), invalid)])],
        )
        .unwrap_err();
        assert_eq!(error.pointer(), "/rows.~1~0/0/x~1y~0z");
        let mismatch = error
            .source()
            .unwrap()
            .downcast_ref::<TypeMismatch>()
            .unwrap();
        assert_eq!(mismatch.expected, ValueType::Float64);
        assert_eq!(mismatch.actual, "non-finite float");
        assert!(matches!(error, OutputEncodeError::InvalidValue { .. }));
    }
    let mut outputs = Outputs::new();
    for finite in [f64::MIN, f64::MAX, f64::MIN_POSITIVE, 1.0, -0.0] {
        encode_output(&mut outputs, "value", finite).unwrap();
        assert!(outputs["value"].is_f64());
        assert_eq!(
            outputs["value"].as_f64().unwrap().to_bits(),
            finite.to_bits()
        );
    }
}

#[test]
fn descriptors_over_the_shared_depth_limit_fail_before_encoding() {
    type L4 = Vec<Vec<Vec<Vec<ValueRef>>>>;
    type L8 = Vec<Vec<Vec<Vec<L4>>>>;
    type L16 = Vec<Vec<Vec<Vec<Vec<Vec<Vec<Vec<L8>>>>>>>>;
    let mut outputs = Outputs::new();
    let error = encode_output(&mut outputs, "deep", None::<L16>).unwrap_err();
    assert_eq!(error.pointer(), "/deep");
    let depth = error
        .source()
        .unwrap()
        .downcast_ref::<TypeDepthError>()
        .unwrap();
    assert_eq!(depth.depth, ValueType::MAX_DEPTH + 1);
    assert!(outputs.is_empty());
    encode_output(&mut outputs, "items", L8::new()).unwrap();
}

#[derive(NodeValue)]
struct Empty {}

#[test]
fn empty_named_structs_emit_empty_maps_and_objects() {
    assert!(<Empty as NodeOutputs>::ports().is_empty());
    assert!(Empty {}.into_outputs().unwrap().is_empty());
    assert_eq!(vec![Empty {}].encode().unwrap(), json!([{}]));
    assert_eq!(Vec::<Empty>::decode(json!([{}]).into()).unwrap().len(), 1);
}

#[derive(Clone, Debug, NodeValue)]
#[value(runtime = "::runtime_alias")]
struct Item<T> {
    #[value(rename = "value/~")]
    value: T,
    label: Option<String>,
    shared: Option<ValueRef>,
}

#[derive(NodeValue)]
struct Nested<T> {
    items: Vec<Item<T>>,
    by_name: BTreeMap<String, Item<T>>,
    item: Item<T>,
    optional: Option<Item<T>>,
}

#[test]
fn derived_values_compose_as_objects_and_collection_elements() {
    let shared = ValueRef::from(json!({"large": [1, 2, 3]}));
    let item = Item {
        value: 42_i64,
        label: None,
        shared: Some(shared.clone()),
    };
    let values = Nested {
        items: vec![item.clone()],
        by_name: BTreeMap::from([("first".into(), item.clone())]),
        item: item.clone(),
        optional: Some(item),
    }
    .into_values()
    .unwrap();
    assert!(
        !values["items"][0]
            .as_object()
            .unwrap()
            .contains_key("label")
    );
    assert_eq!(values["items"][0]["value/~"], json!(42));
    assert!(values["items"][0]["shared"].ptr_eq(&shared));
    assert!(values["by_name"]["first"]["shared"].ptr_eq(&shared));
    assert_eq!(
        <Vec<Item<i64>> as InputValue>::value_type(),
        ValueType::List(Box::new(ValueType::Object))
    );
    assert_eq!(
        <Vec<Item<i64>> as InputValue>::value_type(),
        <Vec<Item<i64>> as OutputValue>::value_type()
    );
    let decoded = Nested::<i64>::from_values(values).unwrap();
    assert_eq!(decoded.items[0].value, 42);
    assert!(decoded.items[0].label.is_none());
    assert!(decoded.items[0].shared.as_ref().unwrap().ptr_eq(&shared));
    assert_eq!(decoded.by_name["first"].value, 42);
    assert_eq!(decoded.item.value, 42);
    assert_eq!(decoded.optional.unwrap().value, 42);
    let mut values = Outputs::new();
    encode_output(&mut values, "optional", None::<Item<i64>>).unwrap();
    assert!(values.is_empty());
    assert!(
        decode_input::<Option<Item<i64>>>(&mut values, "optional")
            .unwrap()
            .is_none()
    );
    assert!(decode_input::<Item<i64>>(&mut values, "required").is_err());
}

#[test]
fn nested_input_failures_preserve_paths_and_structural_causes() {
    for (value, path, actual) in [
        (json!([{"value/~": "bad"}]), "/items/0/value~1~0", "string"),
        (json!([{}]), "/items/0/value~1~0", "missing field"),
        (
            json!([{"value/~": 1, "extra/~": true}]),
            "/items/0/extra~1~0",
            "unknown field",
        ),
        (
            json!([{"value/~": 1, "label": null}]),
            "/items/0/label",
            "null",
        ),
        (json!([null]), "/items/0", "null"),
        (json!([[]]), "/items/0", "array"),
        (json!({}), "/items", "object"),
    ] {
        let error = decode_input::<Vec<Item<i64>>>(
            &mut Outputs::from([("items".into(), value.into())]),
            "items",
        )
        .unwrap_err();
        assert_eq!(error.pointer(), path);
        let mismatch = error
            .source()
            .unwrap()
            .downcast_ref::<TypeMismatch>()
            .unwrap();
        assert_eq!(mismatch.actual, actual);
        if actual == "missing field" || actual == "unknown field" {
            let cause = mismatch
                .source()
                .unwrap()
                .downcast_ref::<Box<InputDecodeError>>()
                .unwrap();
            assert!(matches!(
                cause.as_ref(),
                InputDecodeError::MissingField { .. } | InputDecodeError::UnknownField { .. }
            ));
        }
    }
}

#[test]
fn nested_output_failures_preserve_paths_and_float_causes() {
    let error = encode_output(
        &mut Outputs::new(),
        "items",
        vec![BTreeMap::from([(
            "key/~".into(),
            Item {
                value: f64::INFINITY,
                label: None,
                shared: None,
            },
        )])],
    )
    .unwrap_err();
    assert_eq!(error.pointer(), "/items/0/key~1~0/value~1~0");
    let mismatch = error
        .source()
        .unwrap()
        .downcast_ref::<TypeMismatch>()
        .unwrap();
    assert_eq!(mismatch.expected, ValueType::Float64);
    assert_eq!(mismatch.actual, "non-finite float");
    let cause = mismatch
        .source()
        .unwrap()
        .downcast_ref::<Box<OutputEncodeError>>()
        .unwrap();
    assert!(cause.source().unwrap().is::<TypeMismatch>());
}

#[test]
fn nested_descriptor_failures_retain_depth_errors() {
    type L4 = Vec<Vec<Vec<Vec<ValueRef>>>>;
    type L8 = Vec<Vec<Vec<Vec<L4>>>>;
    type L16 = Vec<Vec<Vec<Vec<Vec<Vec<Vec<Vec<L8>>>>>>>>;
    #[derive(Debug, NodeValue)]
    struct Deep {
        deep: Option<L16>,
    }

    let input = Vec::<Deep>::decode(json!([{}]).into()).unwrap_err();
    let output = vec![Deep { deep: None }].encode().unwrap_err();
    for error in [input, output] {
        assert_eq!(error.path, "/0/deep");
        let depth = error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .downcast_ref::<TypeDepthError>()
            .unwrap();
        assert_eq!(depth.depth, ValueType::MAX_DEPTH + 1);
    }
}
