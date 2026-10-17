use super::*;
use crate::schema::model_calls;
use crate::{ModelIdentity, Reply, nodes::PrepareTitleInputs};

fn identity() -> ModelIdentity {
    ModelIdentity {
        profile: "default".into(),
        model: Some("test-model".into()),
    }
}
fn complete(
    database: &ChatDatabase,
    session: &str,
    usage: Option<BTreeMap<String, i64>>,
) -> crate::Invocation {
    let begun = database
        .begin_turn(session, "question", identity(), None, 1024)
        .unwrap();
    database
        .complete_turn(
            begun.invocation.clone(),
            Reply {
                usages: Some(vec![mfn_llm::CallUsage {
                    started_at: None,
                    call_id: "reported-call".into(),
                    node: None,
                    model: "test-model".into(),
                    usage,
                    error: None,
                }]),
                metadata: None,
                text: "answer".into(),
                finish_reason: None,
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
        .begin_turn(&a.id, "cancelled", identity(), None, 1024)
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
fn pending_titles_do_not_count_but_discarded_title_results_keep_their_usage() {
    let directory = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
    let owner = database.create_session("default", None, None).unwrap();
    complete(&database, &owner.id, None);
    assert_eq!(
        database
            .usage(UsageQuery::default())
            .unwrap()
            .overall
            .attempted_calls,
        1
    );
    database.schedule_titles(10).unwrap();
    let title = database.pending_titles(10).unwrap().pop().unwrap();
    let prepared: crate::nodes::TitleStarted = crate::operation::execute(
        database.path(),
        "mf_chat.title.prepare",
        PrepareTitleInputs {
            database_path: String::new(),
            call_id: title.id.clone(),
            model: identity(),
            max_bytes: 1024,
        },
    )
    .unwrap();
    database.rename_session(&owner.id, "Manual").unwrap();
    let response = Reply {
        usages: Some(vec![mfn_llm::CallUsage {
            started_at: None,
            call_id: "reported-title".into(),
            node: None,
            model: "test-model".into(),
            usage: Some(BTreeMap::from([("output_tokens".into(), 5)])),
            error: None,
        }]),
        metadata: None,
        text: "Generated".into(),
        finish_reason: None,
    };
    database
        .complete_title(
            prepared.invocation.clone(),
            prepared.title_revision,
            response.clone(),
        )
        .unwrap();
    database
        .complete_title(prepared.invocation, prepared.title_revision, response)
        .unwrap();
    let usage = database
        .usage(UsageQuery {
            purpose: Some(CallPurpose::Title),
            group_by: Some(UsageGrouping::Purpose),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(usage.overall.attempted_calls, 1);
    assert_eq!(usage.overall.output_tokens.known_subtotal, Some(5));
    assert_eq!(usage.groups[0].key, "title");
    assert_eq!(database.session(&owner.id).unwrap().title, "Manual");
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
            .begin_turn(&owner.id, "question", identity, None, 1024)
            .unwrap();
        database
            .complete_turn(
                begun.invocation,
                Reply {
                    usages: Some(vec![mfn_llm::CallUsage {
                        started_at: None,
                        call_id: "reported-call".into(),
                        node: None,
                        model: model.into(),
                        usage: Some(BTreeMap::from([("output_tokens".into(), tokens)])),
                        error: None,
                    }]),
                    metadata: None,
                    text: "answer".into(),
                    finish_reason: None,
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
