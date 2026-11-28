use crate::{
    ChatDatabase, ChatError, Completion, Invocation, ModelIdentity, NextTurnQueue,
    PendingMessageId, ToolExecutionMetrics, UncommittedCompletion,
    error::*,
    nodes::{BeginTurnInputs, RecoveryInputs, RecoveryOutputs},
    workflow::ConversationPresentation,
};
use mf_runtime::StreamCancellation;
use mf_telemetry::observation::Observer;
use snafu::{IntoError, OptionExt, ResultExt, ensure};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread::{self, JoinHandle},
};

extern crate mfn_code as _;
extern crate mfn_core as _;

#[derive(Clone)]
pub struct HostConfig {
    pub profiles: BTreeMap<String, crate::WorkflowProfile>,
    pub max_active: usize,
    /// A local observer without export is used when none is supplied.
    pub observer: Option<Observer>,
    pub automatic_titles: bool,
    pub automatic_compaction: crate::CompactionConfig,
}

impl HostConfig {
    pub fn new(profile: crate::WorkflowProfile) -> Self {
        Self {
            profiles: BTreeMap::from([(profile.name.clone(), profile)]),
            max_active: 4,
            observer: None,
            automatic_titles: true,
            automatic_compaction: crate::CompactionConfig::default(),
        }
    }
}

#[derive(Clone)]
struct JobControl {
    steering: Option<crate::SteeringInbox>,
    invocation: Invocation,
    cancellation: StreamCancellation,
    completed: Arc<AtomicBool>,
    tool_metrics: ToolExecutionMetrics,
}

impl JobControl {
    fn new(invocation: Invocation, steering: bool) -> Self {
        Self {
            steering: steering.then(|| crate::SteeringInbox::new(invocation.clone())),
            invocation,
            cancellation: StreamCancellation::default(),
            completed: Arc::default(),
            tool_metrics: ToolExecutionMetrics::default(),
        }
    }
}

type JobResult = Result<Completion, ChatError>;
type JobParts = (JobControl, Receiver<JobResult>);

struct QueuedRequest {
    prompt: String,
    started: Sender<JobParts>,
}

impl AsRef<str> for QueuedRequest {
    fn as_ref(&self) -> &str {
        &self.prompt
    }
}

/// Receipt for one next-turn prompt. Dropping the receipt does not withdraw or cancel it.
pub struct QueuedTurn {
    pub id: PendingMessageId,
    prompt: String,
    started: Receiver<JobParts>,
}

impl QueuedTurn {
    /// Wait for the predecessor to commit and the host to claim this prompt. Like `submit`,
    /// the returned job reports admission and execution errors through `join`.
    pub fn wait(self) -> Result<ChatJob, ChatError> {
        let parts = self.started.recv().ok().context(NextTurnNotStartedSnafu {
            prompt: self.prompt,
        })?;
        Ok(ChatJob::from(parts))
    }
}

/// Normal sending starts a turn if idle and otherwise queues one in the same session.
/// Neither variant steers a runner.
pub enum PromptSubmission {
    Started(ChatJob),
    Queued(QueuedTurn),
}

impl From<ChatJob> for PromptSubmission {
    fn from(job: ChatJob) -> Self {
        Self::Started(job)
    }
}

struct Active {
    control: JobControl,
    next: NextTurnQueue<QueuedRequest>,
}

impl Active {
    fn enqueue(&mut self, prompt: String) -> Result<QueuedTurn, ChatError> {
        ensure!(
            self.control.cancellation.failure().is_none(),
            NextTurnClosedSnafu
        );
        let (sender, started) = mpsc::channel();
        let id = self.next.enqueue(QueuedRequest {
            prompt: prompt.clone(),
            started: sender,
        })?;
        Ok(QueuedTurn {
            id,
            prompt,
            started,
        })
    }
}

#[derive(Default)]
struct Workers {
    closed: bool,
    active: BTreeMap<String, Active>,
    handles: Vec<JoinHandle<()>>,
}

struct Inner {
    database: ChatDatabase,
    config: HostConfig,
    workers: Mutex<Workers>,
    context_windows: crate::ModelContextWindowCache,
}

pub struct ChatHost {
    inner: Arc<Inner>,
}

pub struct ChatJob {
    control: JobControl,
    receiver: Receiver<JobResult>,
    ready: Option<Box<JobResult>>,
}

impl From<JobParts> for ChatJob {
    fn from((control, receiver): JobParts) -> Self {
        Self {
            control,
            receiver,
            ready: None,
        }
    }
}

impl ChatJob {
    pub fn invocation(&self) -> &Invocation {
        &self.control.invocation
    }

    pub fn tool_metrics(&self) -> &ToolExecutionMetrics {
        &self.control.tool_metrics
    }

    /// Keep a handle to recover input not delivered before cancellation or failure.
    pub fn steering(&self) -> Option<&crate::SteeringInbox> {
        self.control.steering.as_ref()
    }

    pub fn steer(&self, text: impl Into<String>) -> Result<crate::PendingMessageId, ChatError> {
        self.steering()
            .context(SteeringClosedSnafu)?
            .submit(text.into())
    }

    pub fn cancel(&self) {
        if let Some(inbox) = self.steering() {
            inbox.close();
        }
        if self.control.completed.load(Ordering::Acquire) {
            return;
        }
        self.control.cancellation.cancel(
            mf_runtime::StreamExecutionSnafu {
                message: "chat request cancelled",
            }
            .build(),
        );
    }

    pub fn poll(&mut self) -> bool {
        if self.ready.is_none() {
            match self.receiver.try_recv() {
                Ok(result) => self.ready = Some(Box::new(result)),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.ready = Some(Box::new(
                        WorkerPanicSnafu {
                            id: &self.control.invocation.call_id,
                        }
                        .fail(),
                    ))
                }
            }
        }
        self.ready.is_some()
    }

    pub fn join(mut self) -> Result<Completion, ChatError> {
        if let Some(result) = self.ready.take() {
            return *result;
        }
        self.receiver.recv().context(WorkerReceiveSnafu)?
    }
}

impl Drop for ChatJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl ChatHost {
    pub fn open(path: impl AsRef<Path>, config: HostConfig) -> Result<Self, ChatError> {
        ensure!(
            config.max_active > 0,
            InvalidSnafu {
                field: "max_active",
                reason: "must be positive",
            }
        );
        ensure!(
            !config.profiles.is_empty(),
            InvalidSnafu {
                field: "profiles",
                reason: "must contain a workflow profile"
            }
        );
        config.automatic_compaction.validate()?;
        for (key, profile) in &config.profiles {
            ensure!(
                key == &profile.name,
                InvalidSnafu {
                    field: "profiles",
                    reason: "keys must match profile names"
                }
            );
        }
        let host = Self {
            inner: Arc::new(Inner {
                database: ChatDatabase::open(path)?,
                config,
                workers: Mutex::new(Workers::default()),
                context_windows: crate::ModelContextWindowCache::default(),
            }),
        };
        Ok(host)
    }

    pub fn database(&self) -> &ChatDatabase {
        &self.inner.database
    }

    pub fn submit(&self, session_id: &str, prompt: &str) -> Result<ChatJob, ChatError> {
        self.start(session_id, prompt, None, None)
    }

    /// Decide between starting and queuing under the same lock as worker handoff.
    pub fn submit_or_queue(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<PromptSubmission, ChatError> {
        self.admit(session_id, prompt, None, None, |active| {
            active.enqueue(prompt.into()).map(PromptSubmission::Queued)
        })
    }

    /// Queue a separate turn in an active session, whether or not its Flow supports steering.
    pub fn queue_next(
        &self,
        session_id: &str,
        prompt: impl Into<String>,
    ) -> Result<QueuedTurn, ChatError> {
        let mut workers = self.inner.workers.lock().unwrap();
        ensure!(!workers.closed, ClosedSnafu);
        let active = workers
            .active
            .get_mut(session_id)
            .context(NextTurnClosedSnafu)?;
        active.enqueue(prompt.into())
    }

    pub fn withdraw_next(&self, session_id: &str, id: PendingMessageId) -> Option<String> {
        self.inner
            .workers
            .lock()
            .unwrap()
            .active
            .get_mut(session_id)?
            .next
            .withdraw(id)
            .map(|request| request.prompt)
    }

    /// Starts a turn with a bounded, best-effort preview subscription.
    /// Unread or discarded previews do not prevent settlement.
    pub fn submit_stream(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<(ChatJob, crate::PreviewReceiver), ChatError> {
        let (consumer, receiver) = crate::PreviewConsumer::channel();
        let job = self.start(session_id, prompt, None, Some(consumer))?;
        Ok((job, receiver))
    }

    /// Retries a turn with the same preview policy as [`Self::submit_stream`].
    pub fn retry_stream(
        &self,
        turn_id: &str,
    ) -> Result<(ChatJob, crate::PreviewReceiver), ChatError> {
        let (original, prompt) = self.database().retry_prompt(turn_id)?;
        let (consumer, receiver) = crate::PreviewConsumer::channel();
        let job = self.start(
            &original.session_id,
            &prompt,
            Some(turn_id.into()),
            Some(consumer),
        )?;
        Ok((job, receiver))
    }

    pub fn retry(&self, turn_id: &str) -> Result<ChatJob, ChatError> {
        let (original, prompt) = self.database().retry_prompt(turn_id)?;
        self.start(&original.session_id, &prompt, Some(turn_id.into()), None)
    }

    fn start(
        &self,
        session_id: &str,
        prompt: &str,
        retry_of: Option<String>,
        preview: Option<crate::PreviewConsumer>,
    ) -> Result<ChatJob, ChatError> {
        self.admit(session_id, prompt, retry_of, preview, |_| {
            SessionBusySnafu { id: session_id }.fail()
        })
    }

    fn admit<R: From<ChatJob>>(
        &self,
        session_id: &str,
        prompt: &str,
        retry_of: Option<String>,
        preview: Option<crate::PreviewConsumer>,
        when_busy: impl FnOnce(&mut Active) -> Result<R, ChatError>,
    ) -> Result<R, ChatError> {
        ensure!(
            !prompt.is_empty(),
            InvalidSnafu {
                field: "prompt",
                reason: "must be nonempty"
            }
        );
        let session = self.database().session(session_id)?;
        ensure!(!session.archived, SessionArchivedSnafu { id: session_id });
        let profile = self
            .inner
            .config
            .profiles
            .get(&session.profile)
            .context(ProfileMissingSnafu {
                name: &session.profile,
            })?
            .clone();
        let mut workers = self
            .inner
            .workers
            .lock()
            .expect("worker state was not poisoned");
        ensure!(!workers.closed, ClosedSnafu);
        if let Some(active) = workers.active.get_mut(session_id) {
            return when_busy(active);
        }
        ensure!(
            workers.active.len() < self.inner.config.max_active,
            CapacitySnafu {
                limit: self.inner.config.max_active
            }
        );
        for handle in workers
            .handles
            .extract_if(.., |handle| handle.is_finished())
        {
            let _ = handle.join();
        }
        let invocation = Invocation::new(session_id);
        let control = JobControl::new(invocation.clone(), profile.backend().supports_steering());
        let (sender, receiver) = mpsc::channel();
        let inner = Arc::clone(&self.inner);
        let request = BeginTurnInputs {
            database_path: self
                .database()
                .path()
                .to_str()
                .context(PathSnafu {
                    path: self.database().path(),
                })?
                .into(),
            session_id: invocation.session_id.clone(),
            turn_id: invocation.turn_id.clone(),
            call_id: invocation.call_id.clone(),
            prompt: prompt.into(),
            model: ModelIdentity {
                profile: profile.name.clone(),
                model: None,
            },
            retry_of,
            result_alternatives: None,
        };
        workers.active.insert(
            session_id.into(),
            Active {
                control: control.clone(),
                next: NextTurnQueue::new(invocation.clone()),
            },
        );
        let worker_control = control.clone();
        let worker = thread::Builder::new()
            .name(format!("mf-chat-{}", invocation.call_id))
            .spawn(move || inner.run_session(request, profile, worker_control, sender, preview));
        match worker {
            Ok(worker) => workers.handles.push(worker),
            Err(source) => {
                workers.active.remove(session_id);
                return Err(WorkerSpawnSnafu.into_error(source));
            }
        }
        Ok(ChatJob::from((control, receiver)).into())
    }

    pub fn retry_completion(
        &self,
        pending: UncommittedCompletion,
    ) -> Result<Completion, ChatError> {
        ensure!(
            !self
                .inner
                .workers
                .lock()
                .expect("worker state was not poisoned")
                .closed,
            ClosedSnafu
        );
        let completion = self.database().save_recovered(pending)?;
        Ok(completion)
    }

    pub fn abandon_completion(&self, invocation: Invocation) -> Result<bool, ChatError> {
        let call = self.database().call(&invocation.call_id)?;
        ensure!(
            call.session_id == invocation.session_id
                && call.turn_id.as_deref() == Some(invocation.turn_id.as_str()),
            ConflictSnafu { id: &call.id }
        );
        self.database()
            .fail_call(&call.id, "cancelled", "uncommitted response abandoned")
    }

    pub fn close(self) -> Result<(), ChatError> {
        self.shutdown()
    }

    fn shutdown(&self) -> Result<(), ChatError> {
        let handles = {
            let mut workers = self
                .inner
                .workers
                .lock()
                .expect("worker state was not poisoned");
            if workers.closed {
                return Ok(());
            }
            workers.closed = true;
            for active in workers.active.values() {
                if let Some(inbox) = &active.control.steering {
                    inbox.close();
                }
                active.control.cancellation.cancel(
                    mf_runtime::StreamExecutionSnafu {
                        message: "chat host closed",
                    }
                    .build(),
                );
            }
            std::mem::take(&mut workers.handles)
        };
        let mut panic = false;
        for handle in handles {
            panic |= handle.join().is_err();
        }
        let _: RecoveryOutputs = crate::turn::recover(RecoveryInputs {
            database_path: self
                .database()
                .path()
                .to_str()
                .context(PathSnafu {
                    path: self.database().path(),
                })?
                .into(),
        })?;
        ensure!(!panic, WorkerPanicSnafu { id: "shutdown" });
        Ok(())
    }
}

impl Drop for ChatHost {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl Inner {
    fn run_session(
        &self,
        mut request: BeginTurnInputs,
        profile: crate::WorkflowProfile,
        mut control: JobControl,
        mut sender: Sender<JobResult>,
        mut preview: Option<crate::PreviewConsumer>,
    ) {
        let mut queued = false;
        loop {
            let admitted = Arc::new(AtomicBool::new(false));
            let flag = admitted.clone();
            let presentation = ConversationPresentation {
                queued_prompt: queued,
                on_admitted: queued
                    .then(|| Box::new(move || flag.store(true, Ordering::Release)) as _),
                preview: preview.take(),
                steering: control.steering.clone(),
                tools: Some(control.tool_metrics.clone()),
                context_windows: self.context_windows.clone(),
            };
            let result = self.run_job(request.clone(), &profile, &control, presentation);
            let result = if queued && !admitted.load(Ordering::Acquire) {
                result
                    .map_err(Box::new)
                    .with_context(|_| NextTurnAdmissionSnafu {
                        prompt: std::mem::take(&mut request.prompt),
                    })
            } else {
                result
            };
            let mut workers = self.workers.lock().unwrap();
            let may_continue = !workers.closed && control.cancellation.failure().is_none();
            let active = workers
                .active
                .get_mut(&request.session_id)
                .expect("session worker owns its slot");
            let next = active
                .next
                .advance(result.as_ref().ok().filter(|_| may_continue));
            control.completed.store(true, Ordering::Release);
            let _ = sender.send(result);
            let Some(next) = next else {
                workers.active.remove(&request.session_id);
                return;
            };
            let invocation = next.invocation;
            let entry = next.message;
            queued = true;
            request.turn_id.clone_from(&invocation.turn_id);
            request.call_id.clone_from(&invocation.call_id);
            request.prompt = entry.prompt;
            request.retry_of = None;
            control = JobControl::new(invocation, control.steering.is_some());
            active.control = control.clone();
            let receiver;
            (sender, receiver) = mpsc::channel();
            // A receipt carries passive parts, not a cancel-on-drop job. Not waiting is not withdrawal.
            let _ = entry.started.send((control.clone(), receiver));
        }
    }

    fn run_job(
        &self,
        request: BeginTurnInputs,
        profile: &crate::WorkflowProfile,
        control: &JobControl,
        presentation: ConversationPresentation,
    ) -> JobResult {
        let invocation = &control.invocation;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.conversation(request, profile, control.cancellation.clone(), presentation)
        }));
        if let Some(inbox) = &control.steering {
            inbox.close();
        }
        control
            .tool_metrics
            .close(control.cancellation.failure().is_some());
        match result {
            Ok(Err(source))
                if !matches!(
                    &source,
                    ChatError::Execution { .. } | ChatError::Uncommitted { .. }
                ) =>
            {
                Err(ExecutionSnafu {
                    invocation: invocation.clone(),
                    cleanup: None,
                }
                .into_error(Box::new(source)))
            }
            Ok(result) => result,
            Err(_) => {
                let original = WorkerPanicSnafu {
                    id: &invocation.call_id,
                }
                .build();
                let cleanup = self
                    .database
                    .fail_turn(invocation.clone(), "failed", "chat worker panicked")
                    .err()
                    .map(Box::new);
                Err(ExecutionSnafu {
                    invocation: invocation.clone(),
                    cleanup,
                }
                .into_error(Box::new(original)))
            }
        }
    }

    fn conversation(
        &self,
        request: BeginTurnInputs,
        profile: &crate::WorkflowProfile,
        cancellation: StreamCancellation,
        presentation: ConversationPresentation,
    ) -> Result<Completion, ChatError> {
        let invocation = Invocation {
            session_id: request.session_id.clone(),
            turn_id: request.turn_id.clone(),
            call_id: request.call_id.clone(),
        };
        let resolution = crate::workflow::run_observed_conversation(
            &self.database,
            request,
            profile.backend(),
            cancellation,
            self.config.automatic_titles,
            &self.config.automatic_compaction,
            self.config.observer.as_ref(),
            presentation,
        )?;
        match resolution.state {
            crate::contract::CompletionState::Committed(completion)
                if completion.result.status == "succeeded" =>
            {
                Ok(completion)
            }
            crate::contract::CompletionState::Committed(completion) => WorkflowFailureSnafu {
                message: crate::turn::failure_summary(
                    completion
                        .result
                        .error
                        .as_deref()
                        .expect("validated failed result has an error"),
                )
                .to_owned(),
            }
            .fail(),
            crate::contract::CompletionState::Uncommitted(pending) => {
                let source = resolution
                    .diagnostics
                    .into_iter()
                    .next()
                    .expect("failed persistence has a source");
                Err(UncommittedSnafu {
                    pending: Box::new(pending),
                }
                .into_error(Box::new(source)))
            }
            crate::contract::CompletionState::Finished(turn) => {
                let source = resolution
                    .diagnostics
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| {
                        WorkflowFailureSnafu {
                            message: turn
                                .error
                                .unwrap_or_else(|| "conversation workflow failed".into()),
                        }
                        .build()
                    });
                Err(ExecutionSnafu {
                    invocation,
                    cleanup: None,
                }
                .into_error(Box::new(source)))
            }
            crate::contract::CompletionState::NotBegun => ConflictSnafu {
                id: invocation.call_id,
            }
            .fail(),
        }
    }
}
