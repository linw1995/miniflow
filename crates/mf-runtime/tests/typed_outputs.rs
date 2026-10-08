extern crate mf_runtime as runtime_alias;
use mf_runtime::{
    NodeInputs, NodeOutputs, NodeValue, OutputEncodeError, Outputs, TypeDepthError, TypeMismatch,
    ValueRef, ValueType, encode_output,
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

#[derive(NodeOutputs)]
struct Empty {}

#[test]
fn empty_named_structs_emit_no_outputs() {
    assert!(<Empty as NodeOutputs>::ports().is_empty());
    assert!(Empty {}.into_outputs().unwrap().is_empty());
}
