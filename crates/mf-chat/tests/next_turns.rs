use mf_chat::{
    ChatError, ChatHost, ConversationBackend, ConversationInputs, ConversationResult, HostConfig,
    Invocation, PromptSubmission, WorkflowProfile,
};
use std::{
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    time::Duration,
};

struct Call {
    invocation: Invocation,
    request: ConversationInputs,
    reply: Sender<ConversationResult>,
}
struct Gated(Sender<Call>);

impl ConversationBackend for Gated {
    fn invoke(
        &self,
        invocation: &Invocation,
        request: ConversationInputs,
        _: &mut mf_runtime::ExecutionContext,
        _: Option<&mut dyn FnMut(mf_chat::PreviewDelta) -> Result<(), ChatError>>,
        _: Option<&mf_telemetry::observation::Observer>,
    ) -> Result<ConversationResult, ChatError> {
        let (reply, answer) = mpsc::channel();
        self.0
            .send(Call {
                invocation: invocation.clone(),
                request,
                reply,
            })
            .unwrap();
        Ok(answer.recv_timeout(Duration::from_secs(10)).unwrap())
    }
}

fn setup() -> (tempfile::TempDir, ChatHost, String, Receiver<Call>) {
    let dir = tempfile::tempdir().unwrap();
    let (tx, calls) = mpsc::channel();
    let profile = WorkflowProfile::with_backend("gated", Arc::new(Gated(tx))).unwrap();
    let host = ChatHost::open(dir.path().join("chat.sqlite"), HostConfig::new(profile)).unwrap();
    let session = host
        .database()
        .create_session("gated", None, None)
        .unwrap()
        .id;
    (dir, host, session, calls)
}

fn receive(calls: &Receiver<Call>, prompt: &str, messages: usize) -> Call {
    let call = calls.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(call.request.prompt().unwrap(), prompt);
    assert_eq!(call.request.messages.len(), messages);
    call
}

#[test]
fn a_session_worker_claims_one_turn_per_commit_without_receipt_polling() {
    let (_dir, host, session, calls) = setup();
    assert!(matches!(
        host.queue_next(&session, "idle"),
        Err(ChatError::NextTurnClosed)
    ));
    let PromptSubmission::Started(first_job) = host.submit_or_queue(&session, "first").unwrap()
    else {
        panic!("idle session queued")
    };
    let first = receive(&calls, "first", 1);
    assert!(first_job.steering().is_none());
    let removed = host.queue_next(&session, "same text").unwrap();
    let PromptSubmission::Queued(second) = host.submit_or_queue(&session, "same text").unwrap()
    else {
        panic!("busy session ran concurrently")
    };
    assert!(host.withdraw_next("other", removed.id).is_none());
    assert_eq!(
        host.withdraw_next(&session, removed.id).as_deref(),
        Some("same text")
    );
    assert!(host.withdraw_next(&session, removed.id).is_none());
    assert!(
        matches!(removed.wait(), Err(ChatError::NextTurnNotStarted { prompt }) if prompt == "same text")
    );
    let third = host.queue_next(&session, "third").unwrap();
    drop(third);
    assert!(calls.try_recv().is_err());
    assert_eq!(
        host.database()
            .messages(&session, 20, None)
            .unwrap()
            .messages
            .len(),
        1
    );
    first
        .reply
        .send(ConversationResult::assistant("one"))
        .unwrap();
    let second_call = receive(&calls, "same text", 3);
    assert_eq!(host.database().model_context(&session).unwrap().revision, 1);
    assert!(host.withdraw_next(&session, second.id).is_none());
    let second_job = second.wait().unwrap();
    assert_eq!(second_job.invocation(), &second_call.invocation);
    assert_ne!(
        second_job.invocation().turn_id,
        first_job.invocation().turn_id
    );
    assert_eq!(first_job.join().unwrap().invocation, first.invocation);
    let fourth = host.queue_next(&session, "fourth").unwrap();
    second_call
        .reply
        .send(ConversationResult::assistant("two"))
        .unwrap();
    let third = receive(&calls, "third", 5);
    third
        .reply
        .send(ConversationResult::assistant("three"))
        .unwrap();
    let fourth_call = receive(&calls, "fourth", 7);
    assert_eq!(
        second_job.join().unwrap().invocation,
        second_call.invocation
    );
    let fourth_job = fourth.wait().unwrap();
    assert_eq!(fourth_job.invocation(), &fourth_call.invocation);
    fourth_call
        .reply
        .send(ConversationResult::assistant("four"))
        .unwrap();
    fourth_job.join().unwrap();
    assert_eq!(host.database().model_context(&session).unwrap().revision, 4);
    assert!(matches!(
        host.queue_next(&session, "too late"),
        Err(ChatError::NextTurnClosed)
    ));
}

#[test]
fn failed_queued_admission_returns_the_claimed_prompt_and_stops_the_tail() {
    let (_dir, host, session, calls) = setup();
    let first_job = host.submit(&session, "first").unwrap();
    let first = receive(&calls, "first", 1);
    let head = host.queue_next(&session, "not admitted").unwrap();
    let tail = host.queue_next(&session, "not claimed").unwrap();
    host.database().archive_session(&session).unwrap();
    first
        .reply
        .send(ConversationResult::assistant("committed"))
        .unwrap();
    first_job.join().unwrap();
    let error = head.wait().unwrap().join().unwrap_err();
    assert!(
        matches!(error, ChatError::NextTurnAdmission { prompt, source } if prompt == "not admitted" && matches!(*source, ChatError::Execution { .. }))
    );
    assert!(
        matches!(tail.wait(), Err(ChatError::NextTurnNotStarted { prompt }) if prompt == "not claimed")
    );
    assert!(calls.try_recv().is_err());
    assert_eq!(
        host.database()
            .messages(&session, 20, None)
            .unwrap()
            .messages
            .len(),
        2
    );
}

#[test]
fn cancellation_and_failed_business_status_do_not_admit_the_next_turn() {
    for cancel in [true, false] {
        let (_dir, host, session, calls) = setup();
        let job = host.submit(&session, "initial").unwrap();
        let first = receive(&calls, "initial", 1);
        let later = host.queue_next(&session, "later").unwrap();
        let mut result = ConversationResult::append(Vec::new());
        result.status = "failed".into();
        result.error = Some("fixture failure".into());
        if cancel {
            job.cancel();
            assert!(matches!(
                host.queue_next(&session, "late"),
                Err(ChatError::NextTurnClosed)
            ));
        }
        first.reply.send(result).unwrap();
        assert!(job.join().is_err());
        assert!(
            matches!(later.wait(), Err(ChatError::NextTurnNotStarted { prompt }) if prompt == "later")
        );
        assert_eq!(host.database().model_context(&session).unwrap().revision, 0);
        assert!(calls.try_recv().is_err());
    }
}
