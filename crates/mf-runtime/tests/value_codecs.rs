use mf_runtime::{
    InputDecodeError, InputValue, Inputs, NodeEnum, NodeInputs, NodeOutputs, NodeValue, NodeValues,
    Nullable, OutputValue, Shared, TypeCompatibility, TypeMismatch, TypedNodeValue, ValueRef,
    ValueType,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    error::Error,
    sync::atomic::{AtomicUsize, Ordering},
};

#[derive(Debug, PartialEq, NodeEnum)]
enum Mode {
    #[value(rename = "fast")]
    Fast,
    #[value(rename = "safe")]
    Safe,
}

fn default_limit() -> usize {
    16
}

#[derive(Debug, PartialEq, NodeEnum)]
#[value(tag = "kind/~", runtime = "mf_runtime")]
enum Command<T> {
    #[value(rename = "stop")]
    Stop,
    #[value(rename = "run")]
    Run {
        #[value(rename = "data/~")]
        data: T,
        #[value(default = "default_limit")]
        limit: usize,
        label: Option<Nullable<String>>,
    },
}

#[derive(Debug, PartialEq, NodeValue)]
struct Message {
    mode: Mode,
    commands: Vec<Command<BTreeMap<String, u64>>>,
}

#[test]
fn enums_round_trip_in_fields_and_collections() {
    let input = json!({
        "mode": "safe",
        "commands": [{"kind/~": "stop"}, {"kind/~": "run", "data/~": {"count": u64::MAX}, "label": null}]
    });
    let message = <Message as InputValue>::decode(input.into()).unwrap();
    assert_eq!(message.mode, Mode::Safe);
    assert_eq!(
        message.commands[1],
        Command::Run {
            data: BTreeMap::from([("count".into(), u64::MAX)]),
            limit: 16,
            label: Some(Nullable::Null),
        }
    );
    let encoded = message.encode().unwrap();
    assert_eq!(encoded["commands"][1]["limit"], json!(16));
    assert_eq!(encoded["commands"][1]["label"], json!(null));
    assert!(encoded["commands"][0].get("label").is_none());
    assert_eq!(<Mode as InputValue>::value_type(), ValueType::String);
    assert_eq!(
        <Command<String> as InputValue>::value_type(),
        ValueType::Object
    );
}

#[test]
fn enum_errors_preserve_field_paths_and_nested_sources() {
    for (value, pointer) in [
        (json!({"kind/~": "missing"}), "/kind~1~0"),
        (json!({"kind/~": 1}), "/kind~1~0"),
        (json!({}), "/kind~1~0"),
        (json!({"kind/~": "run", "data/~": -1}), "/data~1~0"),
        (
            json!({"kind/~": "run", "data/~": 1, "limit": null}),
            "/limit",
        ),
        (json!({"kind/~": "stop", "extra/~": 1}), "/extra~1~0"),
    ] {
        let error = <Command<u64> as InputValue>::decode(value.into()).unwrap_err();
        assert_eq!(error.path, pointer);
        assert!(
            error
                .source()
                .unwrap()
                .downcast_ref::<Box<InputDecodeError>>()
                .is_some()
        );
    }
    assert!(<Mode as InputValue>::decode("unknown".into()).is_err());
    assert!(<Mode as InputValue>::decode(ValueRef::null()).is_err());
    let error = Command::Run {
        data: f32::INFINITY,
        limit: 1,
        label: None,
    }
    .encode()
    .unwrap_err();
    assert_eq!(error.path, "/data~1~0");
    assert!(error.source().is_some());
}

#[derive(Debug, PartialEq, NodeInputs)]
struct Defaults<T> {
    #[input(default)]
    data: T,
    #[input(rename = "size/~", default = "default_limit")]
    size: usize,
}

#[derive(Debug, PartialEq, NodeValue)]
#[value(typed)]
struct DefaultedValue {
    #[value(default = "default_limit")]
    size: usize,
    label: Option<String>,
}

#[test]
fn defaults_change_presence_without_relaxing_supplied_value_validation() {
    assert!(
        Defaults::<String>::ports()
            .iter()
            .all(|port| !port.required)
    );
    assert_eq!(
        Defaults::<String>::from_inputs(Inputs::new()).unwrap(),
        Defaults {
            data: String::new(),
            size: 16
        }
    );
    let supplied = Defaults::<String>::from_inputs(Inputs::from([
        ("data".into(), "present".into()),
        ("size/~".into(), 0_u64.into()),
    ]))
    .unwrap();
    assert_eq!(
        supplied,
        Defaults {
            data: "present".into(),
            size: 0
        }
    );
    let error =
        Defaults::<String>::from_inputs(Inputs::from([("size/~".into(), ValueRef::null())]))
            .unwrap_err();
    assert_eq!(error.pointer(), "/size~1~0");
    let defaulted = DefaultedValue::from_values(NodeValues::new()).unwrap();
    assert_eq!(defaulted.into_values().unwrap()["size"], json!(16));
}

#[derive(Debug, PartialEq, NodeValue)]
#[value(typed)]
struct States {
    field: Option<Nullable<String>>,
    items: Vec<Nullable<u64>>,
}

#[test]
fn nullable_distinguishes_absent_null_and_values_in_both_directions() {
    for (value, field) in [
        (json!({"items": [null, u64::MAX]}), None),
        (
            json!({"field": null, "items": [null, u64::MAX]}),
            Some(Nullable::Null),
        ),
        (
            json!({"field": "text", "items": [null, u64::MAX]}),
            Some(Nullable::Value("text".into())),
        ),
    ] {
        let decoded = <States as InputValue>::decode(value.clone().into()).unwrap();
        assert_eq!(decoded.field, field);
        assert_eq!(decoded.items, [Nullable::Null, Nullable::Value(u64::MAX)]);
        assert_eq!(
            decoded.validate_typed().unwrap().contains(&"field"),
            field.is_some()
        );
        assert_eq!(decoded.encode().unwrap(), value);
    }
    assert!(mf_runtime::decode_input::<Nullable<String>>(&mut Inputs::new(), "required").is_err());
    let error =
        <States as InputValue>::decode(json!({"field": 1, "items": []}).into()).unwrap_err();
    assert_eq!(error.path, "/field");
}

#[test]
fn numeric_codecs_enforce_range_representation_and_finite_values() {
    assert_eq!(
        <u64 as InputValue>::decode(u64::MAX.into()).unwrap(),
        u64::MAX
    );
    assert_eq!(
        <usize as InputValue>::decode((usize::MAX as u64).into()).unwrap(),
        usize::MAX
    );
    for value in [json!(-1), json!(1.0), json!(null), json!("1")] {
        assert!(<u64 as InputValue>::decode(value.clone().into()).is_err());
        assert!(<usize as InputValue>::decode(value.into()).is_err());
    }
    if usize::BITS < 64 {
        assert!(<usize as InputValue>::decode(u64::MAX.into()).is_err());
    }
    for value in [
        f32::MIN,
        f32::MAX,
        -0.0,
        f32::MIN_POSITIVE,
        f32::from_bits(1),
    ] {
        let encoded = value.encode().unwrap();
        assert_eq!(
            <f32 as InputValue>::decode(encoded).unwrap().to_bits(),
            value.to_bits()
        );
    }
    for value in [json!(1), json!(f64::MAX), json!(-f64::MAX), json!(null)] {
        assert!(<f32 as InputValue>::decode(value.into()).is_err());
    }
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let error = vec![Nullable::Value(value)].encode().unwrap_err();
        assert_eq!(error.path, "/0");
        assert_eq!(error.expected, ValueType::Float32);
        assert!(mf_runtime::TypedValueCodec::validate_typed(&value).is_err());
    }
    assert_eq!(u64::MAX.encode().unwrap(), json!(u64::MAX));
    assert_eq!(usize::MAX.encode().unwrap(), json!(usize::MAX));
}

static DECODE_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
struct Counted(Vec<String>);

impl InputValue for Counted {
    fn value_type() -> ValueType {
        <Vec<String> as InputValue>::value_type()
    }
    fn decode(value: ValueRef) -> Result<Self, TypeMismatch> {
        DECODE_COUNT.fetch_add(1, Ordering::SeqCst);
        <Vec<String> as InputValue>::decode(value).map(Self)
    }
}

#[derive(Debug, NodeInputs, NodeOutputs)]
struct Payload {
    data: Shared<Counted>,
}

#[test]
fn shared_payload_defers_decoding_and_preserves_identity() {
    let original = ValueRef::from(json!(["one", "two"]));
    let payload = Payload::from_inputs(Inputs::from([("data".into(), original.clone())])).unwrap();
    assert_eq!(DECODE_COUNT.load(Ordering::SeqCst), 0);
    assert!(payload.data.value().ptr_eq(&original));
    assert_eq!(payload.data.value().as_array().unwrap().len(), 2);
    assert_eq!(payload.data.decode().unwrap().0, ["one", "two"]);
    assert_eq!(DECODE_COUNT.load(Ordering::SeqCst), 1);
    let copy = payload.data.clone();
    let output = payload.into_outputs().unwrap();
    assert!(output["data"].ptr_eq(&original));
    assert!(copy.into_value().ptr_eq(&original));
    assert!(Shared::<Vec<String>>::new(json!([1]).into()).is_err());
    let deferred = Shared::<Message>::new(json!({"mode": "bad"}).into()).unwrap();
    assert_eq!(deferred.decode().unwrap_err().path, "/mode");
}

#[test]
fn new_descriptors_round_trip_and_preserve_checked_assignments() {
    let descriptors = [
        ValueType::Uint64,
        ValueType::Usize,
        ValueType::Float32,
        ValueType::Nullable(Box::new(ValueType::List(Box::new(ValueType::Uint64)))),
    ];
    for descriptor in descriptors {
        let serialized = serde_json::to_value(&descriptor).unwrap();
        assert_eq!(
            serde_json::from_value::<ValueType>(serialized).unwrap(),
            descriptor
        );
    }
    let mut bounded = ValueType::String;
    for _ in 1..ValueType::MAX_DEPTH {
        bounded = ValueType::Nullable(Box::new(bounded));
    }
    bounded.check_depth().unwrap();
    let deep = ValueType::Nullable(Box::new(bounded));
    assert_eq!(
        deep.check_depth().unwrap_err().depth,
        ValueType::MAX_DEPTH + 1
    );
    assert!(ValueType::parse_descriptor(&serde_json::to_value(deep).unwrap()).is_err());
    assert_eq!(
        ValueType::Int64.compatibility_with(&ValueType::Uint64),
        TypeCompatibility::Checked
    );
    assert!(ValueType::Usize.is_assignable_to(&ValueType::Uint64));
    assert!(ValueType::Float32.is_assignable_to(&ValueType::Float64));
    assert_eq!(
        ValueType::Float64.compatibility_with(&ValueType::Float32),
        TypeCompatibility::Checked
    );
    let nullable = ValueType::Nullable(Box::new(ValueType::String));
    assert!(ValueType::Null.is_assignable_to(&nullable));
    assert!(ValueType::String.is_assignable_to(&nullable));
    assert_eq!(
        nullable.compatibility_with(&ValueType::String),
        TypeCompatibility::Checked
    );
    assert_eq!(
        nullable.compatibility_with(&ValueType::Null),
        TypeCompatibility::Checked
    );
}

#[test]
fn nullable_values_reject_noncanonical_null_in_dynamic_and_typed_outputs() {
    fn rejected<T: mf_runtime::TypedValueCodec>(value: Nullable<T>) {
        let typed = mf_runtime::TypedValueCodec::validate_typed(&value).unwrap_err();
        let encoded = value.encode().unwrap_err();
        assert_eq!(typed, encoded);
        assert_eq!(typed.actual, "null wrapped as a value");
    }
    rejected(Nullable::Value(Nullable::<String>::Null));
    rejected(Nullable::Value(
        Shared::<ValueRef>::new(ValueRef::null()).unwrap(),
    ));
    #[derive(NodeValue)]
    #[value(typed)]
    struct Canonical {
        values: Vec<Nullable<ValueRef>>,
    }
    let output = Canonical {
        values: vec![Nullable::Value(ValueRef::null())],
    };
    let typed = output.validate_typed().unwrap_err();
    let encoded = output.into_values().unwrap_err();
    assert_eq!(typed, encoded);
    assert_eq!(typed.pointer(), "/values/0");
    let value = Nullable::Value(ValueRef::from("text"));
    mf_runtime::TypedValueCodec::validate_typed(&value).unwrap();
    assert_eq!(value.encode().unwrap(), json!("text"));
}
