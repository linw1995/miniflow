use crate::{
    ChatDatabase, ConversationResult, ModelIdentity, UsageAttribution, UsageGrouping, UsageQuery,
};

fn begin(database: &ChatDatabase, session_id: &str) -> crate::nodes::TurnStarted {
    database
        .begin_turn(
            session_id,
            "hello",
            ModelIdentity {
                profile: "default".into(),
                model: None,
            },
            None,
        )
        .unwrap()
}

fn result(title: &str) -> ConversationResult {
    ConversationResult {
        text: "answer".into(),
        title: Some(title.into()),
        usages: vec![crate::CallUsage {
            timing: None,
            attribution: Some(UsageAttribution {
                purpose: Some("title".into()),
                profile: Some("summary".into()),
            }),
            started_at: None,
            call_id: "title-call".into(),
            node: None,
            model: "summary-model".into(),
            usage: Some(std::collections::BTreeMap::from([(
                "output_tokens".into(),
                5,
            )])),
            error: None,
        }],
        ..Default::default()
    }
}

#[test]
fn title_and_reply_commit_atomically_and_replay_without_reapplying() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let database = ChatDatabase::open(&path).unwrap();
    let session = database.create_session("default", None, None).unwrap();
    let begun = begin(&database, &session.id);
    let context = begun.request.context.unwrap().title;
    assert_eq!(context.source, "provisional");
    assert!(!context.attempted);
    let completion = database
        .complete_turn(begun.invocation.clone(), result("  Generated  "))
        .unwrap();
    assert_eq!(database.session(&session.id).unwrap().title, "Generated");
    database.rename_session(&session.id, "Manual").unwrap();
    assert_eq!(
        database
            .complete_turn(begun.invocation.clone(), completion.result.clone())
            .unwrap(),
        completion
    );
    assert_eq!(database.session(&session.id).unwrap().title, "Manual");
    let mut conflicting = completion.result.clone();
    conflicting.title = Some("Different".into());
    assert!(
        database
            .complete_turn(begun.invocation.clone(), conflicting)
            .is_err()
    );
    drop(database);
    let reopened = ChatDatabase::open(&path).unwrap();
    assert!(
        matches!(reopened.inspect_invocation(&begun.invocation).unwrap(), crate::contract::InvocationState::Committed(saved) if saved == completion)
    );
    let other = reopened.create_session("default", None, None).unwrap();
    assert!(
        !begin(&reopened, &other.id)
            .request
            .context
            .unwrap()
            .title
            .attempted
    );
    let next = begin(&reopened, &session.id);
    let context = next.request.context.unwrap().title;
    assert!(context.attempted);
    assert_eq!(context.source, "manual");
    assert_eq!(context.revision, 2);
}

#[test]
fn stale_or_rejected_titles_keep_reply_and_per_call_attribution() {
    for title in ["Generated", "\n\t", "bad\ntitle", &"x".repeat(121)] {
        let root = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
        let session = database.create_session("default", None, None).unwrap();
        let begun = begin(&database, &session.id);
        if title == "Generated" {
            database.rename_session(&session.id, "Manual").unwrap();
        }
        let completion = database
            .complete_turn(begun.invocation.clone(), result(title))
            .unwrap();
        assert_eq!(completion.result.title.as_deref(), Some(title));
        let owner = database.session(&session.id).unwrap();
        assert_eq!(
            owner.title_source,
            if title == "Generated" {
                "manual"
            } else {
                "provisional"
            }
        );
        assert_eq!(
            database
                .messages(&session.id, 10, None)
                .unwrap()
                .messages
                .len(),
            2
        );
        for (group, expected) in [
            (UsageGrouping::Purpose, "title"),
            (UsageGrouping::Profile, "summary"),
        ] {
            let usage = database
                .usage(UsageQuery {
                    purpose: Some(crate::CallPurpose::Title),
                    group_by: Some(group),
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(usage.overall.attempted_calls, 1);
            assert_eq!(usage.overall.output_tokens.known_subtotal, Some(5));
            assert_eq!(usage.groups[0].key, expected);
        }
        assert!(
            begin(&database, &session.id)
                .request
                .context
                .unwrap()
                .title
                .attempted
        );
    }
}

#[test]
fn failed_title_usage_is_an_attempt_even_without_a_title_output() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let session = database.create_session("default", None, None).unwrap();
    let begun = begin(&database, &session.id);
    let mut output = result("unused");
    output.title = None;
    output.usages[0].error = Some("provider unavailable".into());
    database.complete_turn(begun.invocation, output).unwrap();
    assert!(
        begin(&database, &session.id)
            .request
            .context
            .unwrap()
            .title
            .attempted
    );
    assert_eq!(
        database.usage(Default::default()).unwrap().overall.statuses["failed"],
        1
    );
}

#[test]
fn upgrading_retires_pending_title_jobs_and_preserves_recorded_attempts() {
    use diesel::{prelude::*, sql_types::Text};
    use diesel_migrations::MigrationHarness;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let database = ChatDatabase::open(&path).unwrap();
    let pending = database.create_session("default", None, None).unwrap();
    let attempted = database.create_session("default", None, None).unwrap();
    let mut connection = crate::storage::connect(&path).unwrap();
    while connection.applied_migrations().unwrap().len() > 4 {
        connection
            .revert_last_migration(crate::storage::MIGRATIONS)
            .unwrap();
    }
    for (id, owner, state) in [
        ("pending", &pending.id, "pending"),
        ("attempted", &attempted.id, "running"),
    ] {
        diesel::sql_query("INSERT INTO model_calls (id, session_id, purpose, profile, state, title_revision, started_at) VALUES (?, ?, 'title', 'default', ?, 0, CASE WHEN ? = 'running' THEN 1 ELSE NULL END)")
            .bind::<Text, _>(id).bind::<Text, _>(owner).bind::<Text, _>(state).bind::<Text, _>(state)
            .execute(&mut connection).unwrap();
    }
    drop(connection);
    drop(database);
    let database = ChatDatabase::open(&path).unwrap();
    assert_eq!(database.call("pending").unwrap().state, "skipped");
    assert_eq!(database.call("attempted").unwrap().state, "interrupted");
    assert!(
        !begin(&database, &pending.id)
            .request
            .context
            .unwrap()
            .title
            .attempted
    );
    assert!(
        begin(&database, &attempted.id)
            .request
            .context
            .unwrap()
            .title
            .attempted
    );
}
