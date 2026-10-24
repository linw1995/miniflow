use mf_chat::{
    ChatDatabase, ChatHost, ConversationBackend, ConversationInputs, ConversationResult,
    HostConfig, Invocation, WorkflowProfile,
};
use mf_runtime::ExecutionContext;
use serde_json::json;
use std::sync::{Arc, Mutex};

struct Backend {
    timestamps: Mutex<Vec<u64>>,
}

impl ConversationBackend for Backend {
    fn invoke(
        &self,
        invocation: &Invocation,
        _: ConversationInputs,
        context: &mut ExecutionContext,
        _: Option<&mut dyn FnMut(mf_chat::TextDelta) -> Result<(), mf_chat::ChatError>>,
        _: Option<&mf_telemetry::observation::Observer>,
    ) -> Result<ConversationResult, mf_chat::ChatError> {
        let mut usage: mf_chat::CallUsage = serde_json::from_value(json!({"call_id":format!("model-{}", invocation.call_id), "model":"fixture", "usage":{"output_tokens":100}})).unwrap();
        let start = context
            .record_event(mfn_llm::REQUEST_STARTED_EVENT, json!(&usage))
            .unwrap();
        let first = context
            .record_event(mfn_llm::FIRST_TEXT_EVENT, json!({"call_id":usage.call_id}))
            .unwrap();
        let end = context
            .record_event(mfn_llm::REQUEST_FINISHED_EVENT, json!(&usage))
            .unwrap();
        self.timestamps.lock().unwrap().extend([start, first, end]);
        assert!(usage.timing.is_none());
        usage.started_at = Some(0);
        Ok(ConversationResult {
            text: "answer".into(),
            usages: vec![usage],
            ..Default::default()
        })
    }
}

#[test]
fn observer_free_hosts_persist_source_event_differences_and_reload_history() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let backend = Arc::new(Backend {
        timestamps: Mutex::new(vec![]),
    });
    let profile = WorkflowProfile::with_backend("fixture", backend.clone()).unwrap();
    let host = ChatHost::open(&path, HostConfig::new(profile)).unwrap();
    let session = host
        .database()
        .create_session("fixture", None, None)
        .unwrap();
    let completion = host.submit(&session.id, "hello").unwrap().join().unwrap();
    let timestamps = backend.timestamps.lock().unwrap();
    let expected = mf_chat::CallTiming {
        elapsed_us: ((timestamps[2] - timestamps[0]) / 1000) as i64,
        first_token_us: Some(((timestamps[1] - timestamps[0]) / 1000) as i64),
    };
    assert_eq!(completion.result.usages[0].timing, Some(expected.clone()));
    assert_eq!(
        completion.result.usages[0].started_at,
        Some((timestamps[0] / 1_000_000) as i64)
    );
    host.close().unwrap();
    let database = ChatDatabase::open(&path).unwrap();
    let messages = database.messages(&session.id, 10, None).unwrap();
    assert_eq!(
        messages.messages[1].usages.as_ref().unwrap()[0].timing,
        Some(expected)
    );
    assert_eq!(
        database
            .usage(Default::default())
            .unwrap()
            .overall
            .performance
            .elapsed
            .reported_calls,
        1
    );
}
