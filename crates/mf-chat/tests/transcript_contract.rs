use mf_chat::{
    ChatError,
    contract::transcript::{
        ConversationResult, MessageUpdate, ResultAlternatives, TerminalResultDecoder,
    },
};
use mf_runtime::{InputValue, NodeValue, NodeValues, OutputValue, ValueRef};
use mfn_llm::ChatMessage;
use serde_json::{Value, json};
use std::error::Error;

fn values(value: Value) -> NodeValues {
    serde_json::from_value(value).unwrap()
}
fn messages(value: Value) -> Vec<ChatMessage> {
    Vec::<ChatMessage>::decode(value.into()).unwrap()
}
fn answer() -> Value {
    json!({"role":"assistant","content":"Answer"})
}
fn exchange() -> Value {
    json!([
        {"role":"assistant","content":"Checking","tool_calls":[
            {"id":"one","name":"lookup","arguments":null},
            {"id":"two","name":"lookup","arguments":[1,true]}
        ]},
        {"role":"tool","tool_call_id":"two","content":""},
        {"role":"tool","tool_call_id":"one","content":"A"},
        answer()
    ])
}
fn append() -> Value {
    json!({"append":{"status":"succeeded","messages":exchange(),"usages":[]}})
}
fn replace() -> Value {
    json!({"replace_context":{"status":"succeeded","context_messages":[],"display_messages":[answer()],"usages":[]}})
}

#[test]
fn named_outputs_are_the_only_wire_tags_and_own_all_their_facts() {
    for wire in [
        append(),
        replace(),
        json!({"append":{
            "status":"failed","messages":[],"usages":[],"error":"Failure"
        }}),
    ] {
        let result = ConversationResult::decode(values(wire.clone())).unwrap();
        assert_eq!(serde_json::to_value(&result).unwrap(), wire);
        assert_eq!(
            serde_json::to_value(result.clone().into_values().unwrap()).unwrap(),
            wire
        );
        assert_eq!(result.clone().encode().unwrap(), wire);
        let nested = <ConversationResult as InputValue>::decode(wire.clone().into()).unwrap();
        assert_eq!(nested, result);
        assert_eq!(
            ConversationResult::from_json(&serde_json::to_vec(&result).unwrap()).unwrap(),
            result
        );
        assert_eq!(
            serde_json::from_value::<ConversationResult>(wire).unwrap(),
            result
        );
    }
    assert_eq!(ConversationResult::ports().len(), 2);
    assert!(
        ConversationResult::ports()
            .iter()
            .all(|port| !port.required)
    );
}

#[test]
fn append_and_replacement_have_distinct_display_and_context_rules() {
    let request = messages(json!([
        {"role":"system","content":"Instructions"},
        {"role":"user","content":"Question"}
    ]));
    let append = ConversationResult::decode(values(append())).unwrap();
    append.validate_against(&request).unwrap();
    assert_eq!(append.display_messages().len(), 4);
    assert_eq!(append.update.candidate_context(&request).len(), 6);
    let replacement = ConversationResult::decode(values(replace())).unwrap();
    replacement.validate_against(&request).unwrap();
    assert!(replacement.update.candidate_context(&request).is_empty());
    assert_eq!(replacement.display_messages(), messages(json!([answer()])));
    assert_eq!(replacement.final_text(), Some("Answer"));
}

#[test]
fn replacement_views_can_overlap_or_have_no_common_suffix() {
    for context in [
        exchange(),
        json!([]),
        json!([
            {"role":"system","content":"New instructions"},
            {"role":"user","content":"Summary only"}
        ]),
    ] {
        let result = ConversationResult::decode(values(json!({"replace_context":{
            "status":"succeeded","context_messages":context,"display_messages":exchange(),"usages":[]
        }}))).unwrap();
        result.validate_against(&messages(exchange())).unwrap();
        assert_eq!(result.display_messages(), messages(exchange()));
        assert_eq!(result.update.candidate_context(&[]), messages(context));
    }
}

#[test]
fn replacement_can_edit_metadata_reorder_results_and_rewrite_coherent_call_ids() {
    let mut context = exchange();
    context[0]["content"] = "Edited preamble".into();
    context[0]["metadata"] = json!({"edited":true,"note":null});
    context[0]["tool_calls"][0]["id"] = "revised-one".into();
    context[0]["tool_calls"][1]["id"] = "revised-two".into();
    context[1]["tool_call_id"] = "revised-two".into();
    context[1]["content"] = "Compact result".into();
    context[2]["tool_call_id"] = "revised-one".into();
    context.as_array_mut().unwrap().swap(1, 2);
    context[3]["content"] = "Edited continuation".into();
    let result = ConversationResult::decode(values(json!({"replace_context":{
        "status":"succeeded","context_messages":context,"display_messages":exchange(),"usages":[]
    }})))
    .unwrap();
    result.validate_against(&messages(exchange())).unwrap();
    assert_eq!(result.display_messages(), messages(exchange()));
    assert_eq!(
        result.update.candidate_context(&[]),
        messages(context.clone())
    );
    for invalid in ["duplicate", "orphan", "unresolved", "interleaved"] {
        let mut broken = context.clone();
        match invalid {
            "duplicate" => broken[0]["tool_calls"][1]["id"] = "revised-one".into(),
            "orphan" => broken[1]["tool_call_id"] = "missing".into(),
            "unresolved" => {
                broken.as_array_mut().unwrap().remove(2);
            }
            "interleaved" => broken.as_array_mut().unwrap().swap(2, 3),
            _ => unreachable!(),
        }
        assert!(ConversationResult::decode(values(json!({"replace_context":{
            "status":"succeeded","context_messages":broken,"display_messages":exchange(),"usages":[]
        }}))).is_err(), "{invalid}");
    }
}

#[test]
fn append_checks_collisions_against_the_admitted_request() {
    let result = ConversationResult::decode(values(append())).unwrap();
    let error = result.validate_against(&messages(exchange())).unwrap_err();
    assert!(matches!(
        error,
        ChatError::Transcript {
            view: "append context",
            ..
        }
    ));
    assert!(error.source().unwrap().is::<mfn_llm::TranscriptError>());
}

#[test]
fn failed_results_can_retain_only_a_valid_partial_display_prefix() {
    let mut prefix = exchange().as_array().unwrap().clone();
    prefix.truncate(2);
    for (tag, lists) in [
        ("append", json!({"messages":prefix})),
        (
            "replace_context",
            json!({"context_messages":[],"display_messages":prefix}),
        ),
    ] {
        let mut payload = json!({"status":"failed","usages":[],"error":"Provider failed"});
        payload
            .as_object_mut()
            .unwrap()
            .extend(lists.as_object().unwrap().clone());
        let result = ConversationResult::decode(values(json!({tag:payload}))).unwrap();
        assert_eq!(result.status, "failed");
        assert_eq!(result.display_messages().len(), 2);
        assert_eq!(result.final_text(), None);
    }
}

#[test]
fn invalid_shapes_and_legacy_aliases_never_supply_missing_branch_facts() {
    for wire in [
        json!({}),
        json!({"status":"succeeded","text":"Answer","usages":[]}),
        json!({"append":null}),
        json!({"append":[]}),
        json!({"append":{},"replace_context":{}}),
        json!({"append":{"status":"succeeded","messages":[answer()]},"usages":[]}),
        json!({"append":{"messages":[answer()],"usages":[]},"status":"succeeded"}),
        json!({"append":{"status":"succeeded","usages":[],"text":"Answer"}}),
        json!({"append":{"status":"succeeded","messages":[answer()],"usages":[],"kind":"append"}}),
        json!({"append":{"status":"succeeded","messages":[answer()],"usages":[],"metadata":{}}}),
        json!({"append":{"status":"succeeded","messages":[answer()],"usages":[],"finish_reason":"stop"}}),
        json!({"replace_context":{"status":"succeeded","context_messages":[],"usages":[]}}),
        json!({"replace_context":{"status":"succeeded","display_messages":[answer()],"usages":[]}}),
        json!({"replace_context":{"status":"succeeded","context_messages":null,"display_messages":[answer()],"usages":[]}}),
        json!({"replace_context":{"status":"succeeded","context_messages":[],"display_messages":[answer()],"usages":[],"extra":true}}),
    ] {
        assert!(
            ConversationResult::decode(values(wire.clone())).is_err(),
            "{wire}"
        );
    }
}

#[test]
fn result_status_and_generated_role_requirements_are_enforced() {
    for payload in [
        json!({"status":"succeeded","messages":[],"usages":[]}),
        json!({"status":"succeeded","messages":[answer()],"usages":[],"error":"Failure"}),
        json!({"status":"failed","messages":[],"usages":[]}),
        json!({"status":"failed","messages":[],"usages":[],"error":" "}),
        json!({"status":"failed","messages":[],"usages":[],"error":"Failure","title":"Title"}),
        json!({"status":"other","messages":[answer()],"usages":[]}),
        json!({"status":"succeeded","messages":[{"role":"user","content":"Echo"},answer()],"usages":[]}),
        json!({"status":"failed","messages":[{"role":"system","content":"Injected"}],"usages":[],"error":"Failure"}),
        json!({"status":"succeeded","messages":[{"role":"assistant","content":"Pending","tool_calls":[{"id":"one","name":"lookup","arguments":{}}]}],"usages":[]}),
        json!({"status":"failed","messages":[{"role":"tool","tool_call_id":"orphan","content":"result"}],"usages":[],"error":"Failure"}),
    ] {
        assert!(
            ConversationResult::decode(values(json!({"append":payload.clone()}))).is_err(),
            "{payload}"
        );
    }
    let pending = json!([{"role":"assistant","content":"","tool_calls":[{"id":"one","name":"lookup","arguments":{}}]}]);
    assert!(ConversationResult::decode(values(json!({"replace_context":{
        "status":"failed","context_messages":pending,"display_messages":[],"usages":[],"error":"Failure"
    }}))).is_err());
}

#[test]
fn malformed_nested_messages_preserve_indexed_decode_sources() {
    let error = ConversationResult::decode(values(json!({"append":{
        "status":"succeeded","messages":[{"role":"assistant","content":"Answer","extra":true}],"usages":[]
    }}))).unwrap_err();
    let ChatError::Decode { source } = error else {
        panic!("expected typed decode failure")
    };
    assert_eq!(source.pointer(), "/append/messages/0/extra");
}

#[test]
fn result_selection_is_invocation_wide_and_invalidates_pending_recovery() {
    for second in [
        append(),
        replace(),
        json!({"append":null}),
        json!({"append":{},"replace_context":{}}),
    ] {
        let mut decoder = TerminalResultDecoder::new(ResultAlternatives::Both);
        decoder.accept(values(append())).unwrap();
        assert!(decoder.result().is_some());
        decoder.accept(values(json!({"debug":42}))).unwrap();
        assert!(decoder.accept(values(second)).is_err());
        assert!(decoder.result().is_none());
        assert!(decoder.accept(values(append())).is_err());
        assert!(decoder.finish().is_err());
    }
}

#[test]
fn single_alternatives_reject_undeclared_results_and_missing_terminal_values() {
    for (allowed, valid, invalid) in [
        (ResultAlternatives::Append, append(), replace()),
        (ResultAlternatives::ReplaceContext, replace(), append()),
    ] {
        let mut decoder = TerminalResultDecoder::new(allowed);
        decoder.accept(values(valid)).unwrap();
        assert!(decoder.finish().is_ok());
        let mut decoder = TerminalResultDecoder::new(allowed);
        assert!(decoder.accept(values(invalid)).is_err());
        assert!(decoder.result().is_none());
        assert!(decoder.finish().is_err());
        let mut decoder = TerminalResultDecoder::new(allowed);
        decoder.accept(values(json!({"debug":42}))).unwrap();
        assert!(decoder.finish().is_err());
    }
}

#[test]
fn canonical_capture_remains_bounded_and_rejects_duplicate_json_keys() {
    assert!(
        ConversationResult::from_json(
            br#"{"append":{"status":"succeeded","status":"failed","messages":[],"usages":[]}}"#
        )
        .is_err()
    );
    assert!(matches!(
        ConversationResult::from_json(&vec![b' '; mf_chat::contract::MAX_RECOVERY_BYTES + 1]),
        Err(ChatError::RecoveryTooLarge { .. })
    ));
    let result = ConversationResult::decode(values(append())).unwrap();
    let nested: ValueRef = result.encode().unwrap();
    assert!(nested.get("update").is_none());
    assert!(matches!(
        ConversationResult::assistant("Answer").update,
        MessageUpdate::Append { .. }
    ));
}
