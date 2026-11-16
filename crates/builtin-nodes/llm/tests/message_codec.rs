use mf_runtime::{FlowValue, InputValue, OutputValue, PreparedSubgraph, ValueRef};
use mfn_llm::{ChatMessage, MessageMetadata, ToolCall, ToolResult};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fmt::Debug};

fn round_trip<T>(wire: Value)
where
    T: InputValue + OutputValue + Serialize + DeserializeOwned + Clone + Debug + PartialEq,
{
    let runtime = T::decode(wire.clone().into()).unwrap();
    let persisted: T = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(runtime, persisted);
    assert_eq!(runtime.clone().encode().unwrap(), wire);
    assert_eq!(serde_json::to_value(&persisted).unwrap(), wire);
    let bytes = serde_json::to_vec(&persisted).unwrap();
    let reloaded: T = mf_runtime::decode_unique_json(&bytes).unwrap();
    assert_eq!(reloaded, runtime);
}

fn reject<T>(wire: Value, path: &str)
where
    T: InputValue + DeserializeOwned + Debug,
{
    let runtime = T::decode(wire.clone().into()).unwrap_err();
    assert_eq!(runtime.path, path, "{wire}");
    let persisted = serde_json::from_value::<T>(wire.clone()).unwrap_err();
    assert!(persisted.to_string().contains(path), "{wire}: {persisted}");
}

#[test]
fn every_role_uses_the_native_canonical_payload() {
    for wire in [
        json!({"role":"system","content":"Be brief"}),
        json!({"role":"user","content":"Look up both","metadata":{
            "source":{"ids":[1,2],"known":true,"missing":null},"label":"user note"
        }}),
        json!({"role":"assistant","content":"Checking","tool_calls":[
            {"id":"one","name":"lookup","arguments":{"key":"a"}},
            {"id":"two","name":"lookup","arguments":{"key":"b"}}
        ]}),
        json!({"role":"assistant","content":"","tool_calls":[
            {"id":"one","name":"lookup","arguments":null}
        ],"metadata":{}}),
        json!({"role":"assistant","content":"Answer","tool_calls":[],"metadata":{}}),
        json!({"role":"assistant","content":"caf\u{e9}\nsecond line"}),
        json!({"role":"tool","tool_call_id":"one","content":""}),
        json!({"role":"tool","tool_call_id":"two","content":"{\"value\":5}","metadata":{
            "origin":"external","nested":[true,false,null,{"count":0}]
        }}),
    ] {
        round_trip::<ChatMessage>(wire);
    }
}

#[test]
fn historical_call_arguments_are_not_restricted_to_objects() {
    for arguments in [
        json!(null),
        json!(true),
        json!(false),
        json!(42),
        json!(u64::MAX),
        json!(-0.0),
        json!(1.5),
        json!("plain arguments"),
        json!([1,true,null,{"key":"value"}]),
        json!({"array":[0,1],"null":null,"large":u64::MAX}),
    ] {
        round_trip::<ToolCall>(json!({"id":"one","name":"lookup","arguments":arguments}));
        round_trip::<ChatMessage>(json!({"role":"assistant","content":"","tool_calls":[
            {"id":"one","name":"lookup","arguments":arguments}
        ]}));
    }
}

#[test]
fn standalone_tool_results_keep_their_distinct_shape_and_empty_content() {
    for content in ["", "5", "{\"error\":\"not found\"}", "line one\nline two"] {
        round_trip::<ToolResult>(json!({"tool_call_id":"one","content":content}));
        let result: ToolResult = serde_json::from_value(json!({
            "tool_call_id":"one","content":content
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(ChatMessage::from(result)).unwrap(),
            json!({"role":"tool","tool_call_id":"one","content":content})
        );
    }
}

#[test]
fn omitted_optionals_and_present_empty_values_remain_distinct() {
    let omitted: ChatMessage = serde_json::from_value(json!({
        "role":"assistant","content":"Answer"
    }))
    .unwrap();
    let present: ChatMessage = serde_json::from_value(json!({
        "role":"assistant","content":"Answer","tool_calls":[],"metadata":{}
    }))
    .unwrap();
    assert_ne!(omitted, present);
    let omitted = serde_json::to_value(omitted).unwrap();
    let present = serde_json::to_value(present).unwrap();
    assert!(omitted.get("metadata").is_none());
    assert!(omitted.get("tool_calls").is_none());
    assert_eq!(present["metadata"], json!({}));
    assert_eq!(present["tool_calls"], json!([]));
}

#[test]
fn json_and_runtime_reject_the_same_role_specific_fields() {
    for (wire, path) in [
        (json!(null), ""),
        (json!([]), ""),
        (json!({"content":"text"}), "/role"),
        (json!({"role":"other","content":"text"}), "/role"),
        (json!({"role":false,"content":"text"}), "/role"),
        (json!({"role":"user"}), "/content"),
        (json!({"role":"user","content":null}), "/content"),
        (json!({"role":"user","content":42}), "/content"),
        (
            json!({"role":"system","content":"text","tool_calls":[]}),
            "/tool_calls",
        ),
        (
            json!({"role":"user","content":"text","tool_call_id":"one"}),
            "/tool_call_id",
        ),
        (
            json!({"role":"assistant","content":"text","tool_call_id":"one"}),
            "/tool_call_id",
        ),
        (
            json!({"role":"assistant","content":"text","tool_calls":null}),
            "/tool_calls",
        ),
        (
            json!({"role":"assistant","content":"text","tool_calls":{}}),
            "/tool_calls",
        ),
        (json!({"role":"tool","content":"text"}), "/tool_call_id"),
        (
            json!({"role":"tool","tool_call_id":false,"content":"text"}),
            "/tool_call_id",
        ),
        (
            json!({"role":"tool","tool_call_id":"one","content":"text","tool_calls":[]}),
            "/tool_calls",
        ),
        (
            json!({"role":"tool","tool_call_id":"one","content":{}}),
            "/content",
        ),
        (
            json!({"role":"user","content":"text","metadata":null}),
            "/metadata",
        ),
        (
            json!({"role":"user","content":"text","metadata":[]}),
            "/metadata",
        ),
        (
            json!({"role":"user","content":"text","extra":true}),
            "/extra",
        ),
        (
            json!({"role":"assistant","content":"","tool_calls":[{}]}),
            "/tool_calls/0/id",
        ),
        (
            json!({"role":"assistant","content":"","tool_calls":[{"id":"one","name":"lookup"}]}),
            "/tool_calls/0/arguments",
        ),
        (
            json!({"role":"assistant","content":"","tool_calls":[{"id":"one","name":"lookup","arguments":{},"extra":true}]}),
            "/tool_calls/0/extra",
        ),
    ] {
        reject::<ChatMessage>(wire, path);
    }
}

#[test]
fn standalone_call_and_result_json_remain_strict() {
    for (wire, path) in [
        (json!(null), ""),
        (json!({"name":"lookup","arguments":{}}), "/id"),
        (json!({"id":false,"name":"lookup","arguments":{}}), "/id"),
        (json!({"id":"one","arguments":{}}), "/name"),
        (json!({"id":"one","name":42,"arguments":{}}), "/name"),
        (json!({"id":"one","name":"lookup"}), "/arguments"),
        (
            json!({"id":"one","name":"lookup","arguments":{},"extra":true}),
            "/extra",
        ),
    ] {
        reject::<ToolCall>(wire, path);
    }
    for (wire, path) in [
        (json!([]), ""),
        (json!({"content":"result"}), "/tool_call_id"),
        (
            json!({"tool_call_id":null,"content":"result"}),
            "/tool_call_id",
        ),
        (json!({"tool_call_id":"one"}), "/content"),
        (json!({"tool_call_id":"one","content":{}}), "/content"),
        (
            json!({"tool_call_id":"one","content":"result","role":"tool"}),
            "/role",
        ),
        (
            json!({"tool_call_id":"one","content":"result","metadata":{}}),
            "/metadata",
        ),
    ] {
        reject::<ToolResult>(wire, path);
    }
}

#[test]
fn complete_message_lists_round_trip_without_reordering_or_flattening() {
    round_trip::<Vec<ChatMessage>>(json!([
        {"role":"system","content":"Use tools"},
        {"role":"user","content":"Calculate"},
        {"role":"assistant","content":"Checking","tool_calls":[
            {"id":"one","name":"lookup","arguments":{"key":"a"}},
            {"id":"two","name":"lookup","arguments":{"key":"b"}}
        ]},
        {"role":"tool","tool_call_id":"two","content":"B"},
        {"role":"tool","tool_call_id":"one","content":"A"},
        {"role":"assistant","content":"Answer","metadata":{"local":true}}
    ]));
}

#[test]
fn external_json_boundaries_reject_duplicate_keys_before_message_decoding() {
    for bytes in [
        br#"{"role":"user","role":"assistant","content":"text"}"#.as_slice(),
        br#"{"role":"user","content":"first","content":"second"}"#,
        br#"{"role":"user","content":"text","metadata":{"key":1,"key":2}}"#,
        br#"{"role":"assistant","content":"","tool_calls":[{"id":"one","id":"two","name":"lookup","arguments":{}}]}"#,
        br#"{"role":"assistant","content":"","tool_calls":[{"id":"one","name":"lookup","arguments":{"key":1,"key":2}}]}"#,
    ] {
        let error = mf_runtime::decode_unique_json::<ChatMessage>(bytes).unwrap_err();
        assert!(error.to_string().contains("duplicate object key"), "{error}");
    }
}

#[test]
fn persistence_keeps_business_payloads_without_mutating_runtime_aliases() {
    let private = ValueRef::from(json!({"token":"private-value","nested":[1,null]})).into_secret();
    let metadata: MessageMetadata = BTreeMap::from([("record".into(), private.clone())]);
    let message = ChatMessage::Assistant {
        content: "Answer".into(),
        tool_calls: Some(vec![ToolCall {
            id: "one".into(),
            name: "lookup".into(),
            arguments: private.clone(),
        }]),
        metadata: Some(metadata),
    };
    let encoded = message.clone().encode().unwrap();
    assert!(encoded["metadata"]["record"].ptr_eq(&private));
    assert!(encoded["tool_calls"][0]["arguments"].ptr_eq(&private));
    let persisted = serde_json::to_value(&message).unwrap();
    assert_eq!(persisted["metadata"]["record"]["token"], "private-value");
    assert_eq!(
        persisted["tool_calls"][0]["arguments"]["token"],
        "private-value"
    );
    assert!(private.is_secret());
    assert!(encoded["metadata"]["record"].is_secret());
    assert!(encoded["tool_calls"][0]["arguments"].is_secret());
}

#[test]
fn persistence_rejects_native_capabilities_instead_of_serializing_placeholders() {
    let flow = FlowValue::new(PreparedSubgraph::new(vec![], vec![], |_| {
        panic!("serialization must not execute a Flow")
    }))
    .unwrap();
    let value = ValueRef::object([("nested".into(), flow.into())]);
    let call = ToolCall {
        id: "one".into(),
        name: "lookup".into(),
        arguments: value.clone(),
    };
    let message = ChatMessage::User {
        content: "Question".into(),
        metadata: Some(BTreeMap::from([("record".into(), value)])),
    };
    for error in [
        serde_json::to_vec(&call).unwrap_err(),
        serde_json::to_vec(&message).unwrap_err(),
    ] {
        assert!(
            error
                .to_string()
                .contains("executable Flow values cannot be serialized"),
            "{error}"
        );
    }
}
