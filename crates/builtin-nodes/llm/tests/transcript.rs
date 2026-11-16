use mf_runtime::InputValue;
use mfn_llm::{
    ChatMessage, LlmExecutionError, LlmInputs, ToolResult, TranscriptError, TranscriptMode,
    call::prepare_request, validate_transcript,
};
use serde_json::{Value, json};
use std::error::Error;

fn messages(wire: Value) -> Vec<ChatMessage> {
    Vec::<ChatMessage>::decode(wire.into()).unwrap()
}

fn batch() -> Value {
    json!({"role":"assistant","content":"","tool_calls":[
        {"id":"one","name":"lookup","arguments":{"key":"a"}},
        {"id":"two","name":"lookup","arguments":["b"]}
    ]})
}

fn result(id: &str) -> Value {
    json!({"role":"tool","tool_call_id":id,"content":""})
}

fn inputs(messages: Vec<ChatMessage>, tool_results: Vec<ToolResult>) -> LlmInputs {
    LlmInputs {
        messages,
        tools: vec![],
        tool_results,
    }
}

#[test]
fn complete_transcripts_validate_without_configuration_and_preserve_their_inputs() {
    for wire in [
        json!([]),
        json!([{"role":"system","content":"Be brief"}]),
        json!([{"role":"user","content":"Question"}]),
        json!([{"role":"assistant","content":"Answer","tool_calls":[]}]),
        json!([batch(), result("two"), result("one")]),
        json!([
            {"role":"system","content":"Use tools"},
            {"role":"user","content":"Question","metadata":{"opaque":[1,true,null]}},
            batch(),result("two"),result("one"),
            {"role":"assistant","content":"Answer"},
            {"role":"system","content":"Updated instructions"},
            {"role":"user","content":"Next question"}
        ]),
        json!([
            {"role":"assistant","content":"","tool_calls":[
                {"id":"wire/id ~value","name":"lookup-1_A","arguments":false}
            ]},
            {"role":"tool","tool_call_id":"wire/id ~value","content":"result"}
        ]),
    ] {
        let messages = messages(wire);
        let original = messages.clone();
        validate_transcript(&messages, TranscriptMode::Complete).unwrap();
        validate_transcript(&messages, TranscriptMode::Partial).unwrap();
        assert_eq!(messages, original);
    }
}

#[test]
fn empty_context_is_valid_but_a_model_request_still_requires_messages() {
    validate_transcript(&[], TranscriptMode::Complete).unwrap();
    validate_transcript(&[], TranscriptMode::Partial).unwrap();
    let error = prepare_request(&inputs(vec![], vec![])).unwrap_err();
    assert!(matches!(
        error,
        LlmExecutionError::InvalidInput { path, .. } if path == "/messages"
    ));
}

#[test]
fn partial_mode_accepts_only_an_unresolved_final_batch() {
    for wire in [
        json!([batch()]),
        json!([batch(), result("one")]),
        json!([batch(), result("two")]),
        json!([
            {"role":"assistant","content":"","tool_calls":[
                {"id":"prior","name":"lookup","arguments":null}
            ]},
            result("prior"),
            {"role":"assistant","content":"Earlier answer"},
            {"role":"user","content":"Question"},
            batch(),result("two")
        ]),
    ] {
        let messages = messages(wire);
        validate_transcript(&messages, TranscriptMode::Partial).unwrap();
        let error = validate_transcript(&messages, TranscriptMode::Complete).unwrap_err();
        assert_eq!(error.path, "/messages");
        assert_eq!(error.requirement, "a result for every pending tool call");
    }
}

#[test]
fn both_modes_reject_invalid_content_names_ids_and_result_pairing() {
    for (wire, path) in [
        (
            json!([{"role":"system","content":""}]),
            "/messages/0/content",
        ),
        (json!([{"role":"user","content":""}]), "/messages/0/content"),
        (
            json!([{"role":"assistant","content":""}]),
            "/messages/0/content",
        ),
        (
            json!([{"role":"assistant","content":"","tool_calls":[]}]),
            "/messages/0/content",
        ),
        (
            json!([{"role":"assistant","content":"","tool_calls":[
                {"id":" \t\n","name":"lookup","arguments":{}}
            ]}]),
            "/messages/0/tool_calls/0/id",
        ),
        (
            json!([{"role":"assistant","content":"","tool_calls":[
                {"id":"one","name":"bad name","arguments":{}}
            ]}]),
            "/messages/0/tool_calls/0/name",
        ),
        (
            json!([{"role":"assistant","content":"","tool_calls":[
                {"id":"one","name":"lookup","arguments":{}},
                {"id":"one","name":"lookup","arguments":{}}
            ]}]),
            "/messages/0/tool_calls/1/id",
        ),
        (json!([result("one")]), "/messages/0/tool_call_id"),
        (
            json!([batch(), result("unknown")]),
            "/messages/1/tool_call_id",
        ),
        (
            json!([batch(), result("one"), result("one")]),
            "/messages/2/tool_call_id",
        ),
        (
            json!([batch(), result("one"), result("two"), result("one")]),
            "/messages/3/tool_call_id",
        ),
        (
            json!([batch(),{"role":"user","content":"Continue"}]),
            "/messages/1",
        ),
        (
            json!([batch(),result("one"),{"role":"system","content":"Continue"}]),
            "/messages/2",
        ),
        (
            json!([batch(),result("two"),{"role":"assistant","content":"Answer"}]),
            "/messages/2",
        ),
        (
            json!([batch(), result("one"), result("two"), batch()]),
            "/messages/3/tool_calls/0/id",
        ),
    ] {
        let messages = messages(wire);
        for mode in [TranscriptMode::Complete, TranscriptMode::Partial] {
            let error = validate_transcript(&messages, mode).unwrap_err();
            assert_eq!(error.path, path, "{mode:?}: {messages:?}");
            assert!(error.to_string().contains(path));
        }
    }
}

#[test]
fn tool_names_share_the_existing_ascii_and_length_rules() {
    for (name, valid) in [
        ("a".to_owned(), true),
        ("_A-01".to_owned(), true),
        ("a".repeat(64), true),
        ("a".repeat(65), false),
        (String::new(), false),
        ("with.dot".to_owned(), false),
        ("with space".to_owned(), false),
        ("caf\u{e9}".to_owned(), false),
        ("with\0null".to_owned(), false),
    ] {
        let messages = messages(json!([
            {"role":"assistant","content":"","tool_calls":[
                {"id":"one","name":name,"arguments":{}}
            ]},
            result("one")
        ]));
        let validation = validate_transcript(&messages, TranscriptMode::Complete);
        assert_eq!(validation.is_ok(), valid, "{name:?}: {validation:?}");
        if let Err(error) = validation {
            assert_eq!(error.path, "/messages/0/tool_calls/0/name");
        }
    }
}

#[test]
fn call_id_scope_is_one_transcript_not_a_shared_context_and_display_namespace() {
    let display = messages(json!([batch(),result("two"),result("one"),
        {"role":"assistant","content":"Answer"}
    ]));
    let mut context = messages(json!([{"role":"system","content":"Be brief"}]));
    context.extend(display.clone());
    validate_transcript(&display, TranscriptMode::Complete).unwrap();
    validate_transcript(&context, TranscriptMode::Complete).unwrap();
    let mut combined = context;
    combined.extend(display);
    let error = validate_transcript(&combined, TranscriptMode::Complete).unwrap_err();
    assert_eq!(error.path, "/messages/5/tool_calls/0/id");
}

#[test]
fn coherent_context_edits_validate_without_comparison_to_the_original_transcript() {
    let original = messages(json!([batch(),result("one"),result("two"),
        {"role":"assistant","content":"Original answer"}
    ]));
    validate_transcript(&original, TranscriptMode::Complete).unwrap();
    let mut edited = original.clone();
    let ChatMessage::Assistant { tool_calls, .. } = &mut edited[0] else {
        panic!("expected assistant calls")
    };
    tool_calls.as_mut().unwrap()[0].id = "rewritten".into();
    let error = validate_transcript(&edited, TranscriptMode::Complete).unwrap_err();
    assert_eq!(error.path, "/messages/1/tool_call_id");
    let ChatMessage::ToolResult { tool_call_id, .. } = &mut edited[1] else {
        panic!("expected tool result")
    };
    *tool_call_id = "rewritten".into();
    validate_transcript(&edited, TranscriptMode::Complete).unwrap();
    validate_transcript(&edited[3..], TranscriptMode::Complete).unwrap();
    validate_transcript(&original, TranscriptMode::Complete).unwrap();
}

#[test]
fn request_preparation_preserves_the_shared_typed_validation_source() {
    let messages = messages(json!([batch(),result("one"),
        {"role":"assistant","content":"Answer too early"}
    ]));
    let expected = validate_transcript(&messages, TranscriptMode::Complete).unwrap_err();
    let error = prepare_request(&inputs(messages, vec![])).unwrap_err();
    assert!(matches!(error, LlmExecutionError::Transcript { .. }));
    assert_eq!(
        error.source().unwrap().downcast_ref::<TranscriptError>(),
        Some(&expected)
    );
}

#[test]
fn explicit_request_results_use_the_same_pairing_and_completion_rules() {
    for (results, expected_path) in [
        (json!([]), "/messages"),
        (
            json!([{"tool_call_id":"one","content":""}]),
            "/tool_results",
        ),
        (
            json!([{"tool_call_id":"unknown","content":""}]),
            "/tool_results/0/tool_call_id",
        ),
        (
            json!([{"tool_call_id":"one","content":""},{"tool_call_id":"one","content":""}]),
            "/tool_results/1/tool_call_id",
        ),
    ] {
        let error = prepare_request(&inputs(
            messages(json!([batch()])),
            Vec::<ToolResult>::decode(results.into()).unwrap(),
        ))
        .unwrap_err();
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<TranscriptError>()
            .unwrap();
        assert_eq!(source.path, expected_path);
    }
    let messages = messages(json!([batch(), result("two")]));
    prepare_request(&inputs(
        messages,
        vec![ToolResult {
            tool_call_id: "one".into(),
            content: String::new(),
        }],
    ))
    .unwrap();
}
