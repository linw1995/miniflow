use mf_telemetry::{
    Count,
    event::LoopPathEntry,
    iteration::{ItemIdentity, IterationContext},
};

fn context() -> IterationContext {
    IterationContext {
        invocation: Count::try_from(2).unwrap(),
        iteration_path: vec!["repeat".into(), "each".into()],
        body_path: vec!["repeat".into(), "each".into(), "call".into()],
        parent_path: vec![LoopPathEntry::Loop {
            loop_id: "repeat".into(),
            index: Count::try_from(3).unwrap(),
        }],
        parent_stream_invocation: Some(u64::MAX),
        index: Count::try_from(4).unwrap(),
        sequence: Count::try_from(5).unwrap(),
        elapsed_ns: Count::try_from(100).unwrap(),
    }
}

#[test]
fn iteration_context_round_trips_scopes_and_dynamic_identities() {
    let context = context();
    let json = serde_json::to_string(&context).unwrap();
    assert_eq!(IterationContext::from_json(&json).unwrap(), context);
    assert_eq!(
        context.item(),
        ItemIdentity {
            invocation: Count::try_from(2).unwrap(),
            index: Count::try_from(4).unwrap()
        }
    );
}

#[test]
fn malformed_and_unbounded_contexts_are_rejected() {
    let value = serde_json::to_value(context()).unwrap();
    for (field, invalid) in [
        ("invocation", serde_json::json!(0)),
        ("sequence", serde_json::json!(0)),
        ("index", serde_json::json!(-1)),
        ("body_path", serde_json::json!(["unrelated"])),
        ("iteration_path", serde_json::json!([])),
        (
            "parent_path",
            serde_json::json!([{"loop_id":"other","index":0}]),
        ),
    ] {
        let mut invalid_context = value.clone();
        invalid_context[field] = invalid;
        assert!(
            IterationContext::from_json(&invalid_context.to_string()).is_err(),
            "{field}"
        );
    }
    let mut too_deep = context();
    too_deep
        .body_path
        .resize(mf_telemetry::MAX_SCOPE_DEPTH + 1, "scope".into());
    assert!(IterationContext::from_json(&serde_json::to_string(&too_deep).unwrap()).is_err());
    assert!(IterationContext::from_json(&" ".repeat(32 * 1024 + 1)).is_err());
    let error = IterationContext::from_json("{").unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
}
