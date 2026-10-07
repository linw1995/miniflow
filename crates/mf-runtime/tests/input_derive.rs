use mf_runtime::{InputDecodeError, Inputs, NodeInputs, ValueRef, ValueType};
use serde_json::json;
use std::collections::BTreeMap;

type Path = Option<String>;

#[derive(Debug, NodeInputs)]
struct RequestInputs<T> {
    url: String,
    headers: Option<BTreeMap<String, String>>,
    body: Option<ValueRef>,
    path: Path,
    #[input(rename = "rows./~")]
    rows: Vec<BTreeMap<String, i64>>,
    r#type: bool,
    payload: T,
}

#[derive(Debug, NodeInputs)]
#[input(runtime = "mf_runtime")]
struct Empty {}

#[derive(Debug, NodeInputs, serde::Deserialize)]
struct IndependentNames {
    #[serde(rename = "serialized_url")]
    url: String,
}

#[test]
fn serde_attributes_do_not_change_input_declarations() {
    assert_eq!(IndependentNames::ports()[0].name, "url");
    let input =
        IndependentNames::from_inputs(Inputs::from([("url".into(), "input".into())])).unwrap();
    assert_eq!(input.url, "input");
    let decoded: IndependentNames =
        serde_json::from_value(json!({"serialized_url": "serde"})).unwrap();
    assert_eq!(decoded.url, "serde");
}

#[test]
fn declarations_and_decoding_share_field_identity_and_presence() {
    let ports = RequestInputs::<ValueRef>::ports();
    assert_eq!(
        ports
            .iter()
            .map(|port| port.name.as_ref())
            .collect::<Vec<_>>(),
        [
            "url", "headers", "body", "path", "rows./~", "type", "payload"
        ]
    );
    assert_eq!(
        ports.iter().map(|port| port.required).collect::<Vec<_>>(),
        [true, false, false, false, true, true, true]
    );
    assert_eq!(
        ports[4].value_type,
        ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Int64))))
    );
    let payload = ValueRef::from(json!({"raw": [1, 2]}));
    let inputs = Inputs::from([
        ("url".into(), "https://example.test".into()),
        ("body".into(), ValueRef::null()),
        ("rows./~".into(), json!([{"count": 3}]).into()),
        ("type".into(), true.into()),
        ("payload".into(), payload.clone()),
    ]);
    let request = RequestInputs::<ValueRef>::from_inputs(inputs.clone()).unwrap();
    assert_eq!(request.url, "https://example.test");
    assert!(request.headers.is_none());
    assert!(request.body.unwrap().is_null());
    assert!(request.path.is_none());
    assert_eq!(request.rows[0]["count"], 3);
    assert!(request.r#type);
    assert!(request.payload.ptr_eq(&payload));
    let mut null_path = inputs;
    null_path.insert("path".into(), ValueRef::null());
    assert!(
        matches!(RequestInputs::<ValueRef>::from_inputs(null_path), Err(InputDecodeError::InvalidValue { port, .. }) if port == "path")
    );
}

#[test]
fn empty_inputs_reject_unknown_keys_and_regular_inputs_reject_missing_fields() {
    assert!(Empty::ports().is_empty());
    Empty::from_inputs(Inputs::new()).unwrap();
    let error = Empty::from_inputs(Inputs::from([("a/b~c".into(), true.into())])).unwrap_err();
    assert_eq!(error.pointer(), "/a~1b~0c");
    assert!(
        matches!(RequestInputs::<i64>::from_inputs(Inputs::new()), Err(InputDecodeError::MissingField { port }) if port == "url")
    );
}
