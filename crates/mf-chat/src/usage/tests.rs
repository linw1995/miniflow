use super::*;
use crate::schema::model_calls;
use crate::{ConversationResult, ModelIdentity};

fn identity() -> ModelIdentity {
    ModelIdentity {
        profile: "default".into(),
        model: Some("test-model".into()),
    }
}

#[test]
fn performance_uses_paired_weighted_facts_and_survives_reopen_and_replay() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let database = ChatDatabase::open(&path).unwrap();
    let session = database.create_session("default", None, None).unwrap();
    let begun = database
        .begin_turn(&session.id, "question", identity(), None)
        .unwrap();
    let cases = [
        ("stream", Some((2_000_000, Some(500_000))), Some(100), None),
        (
            "stream",
            Some((4_000_000, Some(1_000_000))),
            Some(100),
            None,
        ),
        ("unary", Some((1_000_000, None)), Some(60), None),
        ("sparse", Some((3_000_000, Some(1_000_000))), None, None),
        ("zero", Some((1_000_000, Some(1_000_000))), Some(0), None),
        (
            "failed",
            Some((5_000_000, Some(2_000_000))),
            Some(1000),
            Some("provider failed"),
        ),
        ("legacy", None, Some(500), None),
    ];
    let usages = cases
        .into_iter()
        .enumerate()
        .map(|(index, (model, timing, tokens, error))| crate::CallUsage {
            attribution: None,
            started_at: Some(1000 + index as i64),
            timing: timing.map(|(elapsed_us, first_token_us)| crate::CallTiming {
                elapsed_us,
                first_token_us,
            }),
            call_id: format!("call-{index}"),
            node: None,
            model: model.into(),
            usage: tokens.map(|tokens| BTreeMap::from([("output_tokens".into(), tokens)])),
            error: error.map(Into::into),
        })
        .collect::<Vec<_>>();
    let in_memory = UsageSummary::from_calls(&usages).unwrap();
    let reply = ConversationResult {
        text: "answer".into(),
        usages: usages.clone(),
        ..Default::default()
    };
    database
        .complete_turn(begun.invocation.clone(), reply.clone())
        .unwrap();
    let before = database
        .usage(UsageQuery {
            group_by: Some(UsageGrouping::Model),
            ..Default::default()
        })
        .unwrap();
    database
        .complete_turn(begun.invocation.clone(), reply)
        .unwrap();
    assert_eq!(
        database.call(&begun.invocation.call_id).unwrap().usages,
        Some(usages.clone())
    );
    drop(database);
    let database = ChatDatabase::open(&path).unwrap();
    let after = database
        .usage(UsageQuery {
            group_by: Some(UsageGrouping::Model),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(in_memory, after.overall);
    let first_page = database.messages(&session.id, 1, None).unwrap();
    assert!(first_page.messages[0].usages.is_none());
    let assistant_page = database
        .messages(&session.id, 1, first_page.next_after)
        .unwrap();
    assert_eq!(assistant_page.messages.len(), 1);
    assert_eq!(assistant_page.messages[0].usages, Some(usages));
    let performance = &after.overall.performance;
    assert_eq!(after.overall.attempted_calls, 7);
    assert_eq!(performance.elapsed.total_us, Some(16_000_000));
    assert_eq!(performance.elapsed.reported_calls, 6);
    assert_eq!(performance.elapsed.missing_calls, 1);
    assert_eq!(performance.first_token.mean_ms, Some(1100.0));
    assert_eq!(performance.first_token.reported_calls, 5);
    assert_eq!(performance.first_token.missing_calls, 2);
    assert_eq!(performance.end_to_end.output_tokens, Some(260));
    assert_eq!(performance.end_to_end.elapsed_us, Some(8_000_000));
    assert_eq!(performance.end_to_end.tokens_per_second, Some(32.5));
    assert_eq!(performance.end_to_end.reported_calls, 4);
    assert_eq!(performance.end_to_end.missing_calls, 3);
    assert_eq!(performance.generation.reported_calls, 2);
    assert_eq!(performance.generation.missing_calls, 5);
    assert!((performance.generation.tokens_per_second.unwrap() - 200.0 / 4.5).abs() < 1e-9);
    let zero = &after
        .groups
        .iter()
        .find(|group| group.key == "zero")
        .unwrap()
        .summary
        .performance;
    assert_eq!(zero.end_to_end.tokens_per_second, Some(0.0));
    assert_eq!(zero.generation.tokens_per_second, None);
    let stream = database
        .usage(UsageQuery {
            start_ms: Some(1000),
            end_ms: Some(1002),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(stream.overall.attempted_calls, 2);
    assert_eq!(stream.overall.performance.first_token.mean_ms, Some(750.0));
    let sparse = database
        .usage(UsageQuery {
            model: Some("sparse".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(sparse.overall.performance.end_to_end.elapsed_us, None);
    assert_eq!(sparse.overall.performance.end_to_end.missing_calls, 1);
}

#[test]
fn invalid_timing_is_rejected_before_persistence_and_overflow_is_explicit() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let session = database.create_session("default", None, None).unwrap();
    let begun = database
        .begin_turn(&session.id, "question", identity(), None)
        .unwrap();
    let mut usage = crate::CallUsage {
        attribution: None,
        started_at: None,
        timing: None,
        call_id: "call".into(),
        node: None,
        model: "model".into(),
        usage: None,
        error: None,
    };
    for (elapsed_us, first_token_us) in [(-1, None), (1, Some(-1)), (1, Some(2))] {
        usage.timing = Some(crate::CallTiming {
            elapsed_us,
            first_token_us,
        });
        let reply = ConversationResult {
            text: "answer".into(),
            usages: vec![usage.clone()],
            ..Default::default()
        };
        assert!(matches!(
            database.complete_turn(begun.invocation.clone(), reply),
            Err(ChatError::Invalid {
                field: "call timing",
                ..
            })
        ));
        assert!(
            database
                .call(&begun.invocation.call_id)
                .unwrap()
                .usages
                .is_none()
        );
    }
    usage.timing = Some(crate::CallTiming {
        elapsed_us: i64::MAX,
        first_token_us: None,
    });
    let mut failed = usage.clone();
    failed.call_id = "failed".into();
    failed.timing.as_mut().unwrap().elapsed_us = 1;
    failed.error = Some("provider failed".into());
    database
        .complete_turn(
            begun.invocation,
            ConversationResult {
                text: "answer".into(),
                usages: vec![usage, failed],
                ..Default::default()
            },
        )
        .unwrap();
    assert!(matches!(
        database.usage(Default::default()),
        Err(ChatError::Arithmetic { .. })
    ));
}
fn complete(
    database: &ChatDatabase,
    session: &str,
    usage: Option<BTreeMap<String, i64>>,
) -> crate::Invocation {
    let begun = database
        .begin_turn(session, "question", identity(), None)
        .unwrap();
    database
        .complete_turn(
            begun.invocation.clone(),
            ConversationResult {
                usages: vec![crate::CallUsage {
                    timing: None,
                    attribution: None,
                    started_at: None,
                    call_id: "reported-call".into(),
                    node: None,
                    model: "test-model".into(),
                    usage,
                    error: None,
                }],
                metadata: None,
                text: "answer".into(),
                finish_reason: None,
                ..Default::default()
            },
        )
        .unwrap();
    begun.invocation
}

#[test]
fn sparse_usage_and_failed_attempts_preserve_unknown_counters_and_group_totals() {
    let directory = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
    let a = database
        .create_session("default", None, Some("A".into()))
        .unwrap();
    let b = database
        .create_session("default", None, Some("B".into()))
        .unwrap();
    complete(
        &database,
        &a.id,
        Some(BTreeMap::from([
            ("input_tokens".into(), 2),
            ("output_tokens".into(), 1),
            ("total_tokens".into(), 3),
        ])),
    );
    complete(
        &database,
        &a.id,
        Some(BTreeMap::from([("output_tokens".into(), 3)])),
    );
    let failed = database
        .begin_turn(&a.id, "cancelled", identity(), None)
        .unwrap();
    database
        .fail_turn(failed.invocation, "cancelled", "cancelled")
        .unwrap();
    complete(&database, &b.id, None);
    let first = database
        .usage(UsageQuery {
            group_by: Some(UsageGrouping::Session),
            limit: 1,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(first.overall.attempted_calls, 4);
    assert_eq!(
        first.overall.input_tokens,
        TokenCounter {
            known_subtotal: Some(2),
            reported_calls: 1,
            missing_calls: 3
        }
    );
    assert_eq!(
        first.overall.output_tokens,
        TokenCounter {
            known_subtotal: Some(4),
            reported_calls: 2,
            missing_calls: 2
        }
    );
    assert_eq!(first.overall.statuses["cancelled"], 1);
    assert_eq!(first.groups.len(), 1);
    let second = database
        .usage(UsageQuery {
            group_by: Some(UsageGrouping::Session),
            limit: 1,
            after: first.next_after,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(first.overall, second.overall);
    assert_eq!(second.groups.len(), 1);
    assert!(second.next_after.is_none());
    let scoped = database
        .usage(UsageQuery {
            session_id: Some(a.id),
            purpose: Some(CallPurpose::Conversation),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(scoped.overall.attempted_calls, 3);
}

#[test]
fn zero_empty_and_overflow_are_distinct_from_unknown_usage() {
    let directory = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
    let owner = database
        .create_session("default", None, Some("Fixed".into()))
        .unwrap();
    let empty = database.usage(UsageQuery::default()).unwrap();
    assert_eq!(empty.overall.attempted_calls, 0);
    assert_eq!(empty.overall.output_tokens.known_subtotal, None);
    complete(
        &database,
        &owner.id,
        Some(BTreeMap::from([("output_tokens".into(), 0)])),
    );
    let zero = database.usage(UsageQuery::default()).unwrap();
    assert_eq!(zero.overall.output_tokens.known_subtotal, Some(0));
    assert_eq!(zero.overall.output_tokens.reported_calls, 1);
    complete(
        &database,
        &owner.id,
        Some(BTreeMap::from([("output_tokens".into(), i64::MAX)])),
    );
    complete(
        &database,
        &owner.id,
        Some(BTreeMap::from([("output_tokens".into(), 1)])),
    );
    assert!(database.usage(UsageQuery::default()).is_err());
}

#[test]
fn utc_start_bounds_are_inclusive_exclusive_and_group_pages_do_not_change_totals() {
    let directory = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
    let owner = database
        .create_session("default", None, Some("Fixed".into()))
        .unwrap();
    let first = complete(&database, &owner.id, None);
    let second = complete(&database, &owner.id, None);
    let mut connection = storage::connect(database.path()).unwrap();
    diesel::update(model_calls::table.find(first.call_id))
        .set(model_calls::started_at.eq(Some(86_399_999i64)))
        .execute(&mut connection)
        .unwrap();
    diesel::update(model_calls::table.find(second.call_id))
        .set(model_calls::started_at.eq(Some(86_400_000i64)))
        .execute(&mut connection)
        .unwrap();
    let day = database
        .usage(UsageQuery {
            group_by: Some(UsageGrouping::Day),
            limit: 1,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(day.groups[0].key, "1970-01-01");
    assert_eq!(day.overall.attempted_calls, 2);
    let next = database
        .usage(UsageQuery {
            group_by: Some(UsageGrouping::Day),
            limit: 1,
            after: day.next_after,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(next.groups[0].key, "1970-01-02");
    assert_eq!(day.overall, next.overall);
    let before = database
        .usage(UsageQuery {
            end_ms: Some(86_400_000),
            ..Default::default()
        })
        .unwrap();
    let after = database
        .usage(UsageQuery {
            start_ms: Some(86_400_000),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(before.overall.attempted_calls, 1);
    assert_eq!(after.overall.attempted_calls, 1);
    assert!(
        database
            .usage(UsageQuery {
                start_ms: Some(2),
                end_ms: Some(1),
                ..Default::default()
            })
            .is_err()
    );
}

#[test]
fn model_profile_and_purpose_groups_preserve_attribution_and_validate_cursors() {
    let directory = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
    for (profile, model, tokens) in [("alpha", "model-a", 1), ("beta", "model-b", 2)] {
        let owner = database
            .create_session(profile, None, Some("Manual".into()))
            .unwrap();
        let mut identity = identity();
        identity.profile = profile.into();
        identity.model = Some(model.into());
        let begun = database
            .begin_turn(&owner.id, "question", identity, None)
            .unwrap();
        database
            .complete_turn(
                begun.invocation,
                ConversationResult {
                    usages: vec![crate::CallUsage {
                        timing: None,
                        attribution: None,
                        started_at: None,
                        call_id: "reported-call".into(),
                        node: None,
                        model: model.into(),
                        usage: Some(BTreeMap::from([("output_tokens".into(), tokens)])),
                        error: None,
                    }],
                    metadata: None,
                    text: "answer".into(),
                    finish_reason: None,
                    ..Default::default()
                },
            )
            .unwrap();
    }
    for (grouping, expected) in [
        (UsageGrouping::Model, vec!["model-a", "model-b"]),
        (UsageGrouping::Profile, vec!["alpha", "beta"]),
        (UsageGrouping::Purpose, vec!["conversation"]),
    ] {
        let result = database
            .usage(UsageQuery {
                group_by: Some(grouping),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            result
                .groups
                .iter()
                .map(|group| group.key.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(result.overall.output_tokens.known_subtotal, Some(3));
        assert_eq!(
            result
                .groups
                .iter()
                .map(|group| group.summary.output_tokens.known_subtotal.unwrap())
                .sum::<i64>(),
            3
        );
    }
    let scoped = database
        .usage(UsageQuery {
            profile: Some("alpha".into()),
            model: Some("model-a".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(scoped.overall.attempted_calls, 1);
    assert_eq!(scoped.overall.output_tokens.known_subtotal, Some(1));
    assert!(matches!(
        query(UsageInputs {
            database_path: database.path().to_str().unwrap().into(),
            query: UsageQuery {
                after: Some("cursor".into()),
                ..Default::default()
            },
        }),
        Err(ChatError::Invalid { field: "after", .. })
    ));
}

#[test]
fn historical_usage_keeps_unknown_timing_and_failure_rates_are_not_successes() {
    let mut usage: crate::CallUsage = serde_json::from_value(serde_json::json!({
        "call_id":"old-call", "model":"model", "usage":{"output_tokens":300}
    }))
    .unwrap();
    assert!(usage.timing.is_none());
    assert_eq!(usage.token_rates(), CallTokenRates::default());
    assert!(
        serde_json::to_value(&usage)
            .unwrap()
            .get("timing")
            .is_none()
    );
    usage.timing = Some(CallTiming {
        elapsed_us: 2_000_000,
        first_token_us: Some(500_000),
    });
    assert_eq!(
        usage.token_rates().end_to_end_tokens_per_second,
        Some(150.0)
    );
    let restored: crate::CallUsage =
        serde_json::from_slice(&serde_json::to_vec(&usage).unwrap()).unwrap();
    assert_eq!(restored, usage);
    usage.error = Some("provider failed".into());
    assert_eq!(usage.token_rates(), CallTokenRates::default());
    assert!(usage.timing.is_some());
}

#[test]
fn rates_use_reported_tokens_and_distinct_observed_windows() {
    let timing = CallTiming {
        elapsed_us: 2_000_000,
        first_token_us: Some(500_000),
    };
    assert_eq!(
        timing.token_rates(Some(300)),
        CallTokenRates {
            end_to_end_tokens_per_second: Some(150.0),
            generation_tokens_per_second: Some(200.0),
        }
    );
    assert_eq!(
        timing.token_rates(Some(0)).generation_tokens_per_second,
        Some(0.0)
    );
    assert_eq!(timing.token_rates(None), CallTokenRates::default());
    assert_eq!(timing.token_rates(Some(-1)), CallTokenRates::default());
    let unary = CallTiming {
        first_token_us: None,
        ..timing.clone()
    };
    assert_eq!(
        unary.token_rates(Some(300)).end_to_end_tokens_per_second,
        Some(150.0)
    );
    assert_eq!(
        unary.token_rates(Some(300)).generation_tokens_per_second,
        None
    );
}

#[test]
fn invalid_and_zero_windows_do_not_invent_rates() {
    for timing in [
        CallTiming {
            elapsed_us: -1,
            first_token_us: None,
        },
        CallTiming {
            elapsed_us: 1,
            first_token_us: Some(-1),
        },
        CallTiming {
            elapsed_us: 1,
            first_token_us: Some(2),
        },
    ] {
        assert!(!timing.is_valid());
        assert_eq!(timing.token_rates(Some(1)), CallTokenRates::default());
    }
    let zero = CallTiming {
        elapsed_us: 0,
        first_token_us: Some(0),
    };
    assert!(zero.is_valid());
    assert_eq!(zero.token_rates(Some(0)), CallTokenRates::default());
    let single = CallTiming {
        elapsed_us: 10,
        first_token_us: Some(10),
    };
    assert_eq!(
        single.token_rates(Some(1)).generation_tokens_per_second,
        None
    );
}

#[test]
fn raw_call_facts_and_enriched_application_values_keep_the_flat_wire_contract() {
    let raw_json = serde_json::json!({
        "call_id":"call", "model":"fixture", "started_at":42,
        "attribution":{"purpose":"conversation","profile":"fixture"},
        "node":{"id":"model","kind":"builtin.llm","path":[{"node":"loop","index":1}]},
        "usage":{"output_tokens":100}
    });
    let facts: mfn_llm::CallUsage = serde_json::from_value(raw_json.clone()).unwrap();
    assert!(
        mfn_llm::CallUsage::ports()
            .iter()
            .all(|port| port.name != "timing")
    );
    let mut usage = crate::CallUsage::from(facts);
    assert!(usage.timing.is_none());
    assert_eq!(serde_json::to_value(&usage).unwrap(), raw_json);
    usage.timing = Some(CallTiming {
        elapsed_us: 2_000_000,
        first_token_us: Some(500_000),
    });
    let mut expected = raw_json;
    expected["timing"] = serde_json::json!({"elapsed_us":2_000_000,"first_token_us":500_000});
    assert_eq!(serde_json::to_value(&usage).unwrap(), expected);
    let legacy: crate::CallUsage = serde_json::from_value(expected).unwrap();
    assert_eq!(legacy, usage);
    assert_eq!(
        crate::CallUsage::from_values(usage.clone().into_values().unwrap()).unwrap(),
        usage
    );
    assert_eq!(usage.token_rates().end_to_end_tokens_per_second, Some(50.0));
}
