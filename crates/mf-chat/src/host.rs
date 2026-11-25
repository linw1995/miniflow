use crate::{
    ChatDatabase, ChatError, Completion, Invocation, ModelIdentity, ToolExecutionMetrics,
    UncommittedCompletion,
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
        mpsc::{self, Receiver, TryRecvError},
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

struct Active {
    steering: Option<crate::SteeringInbox>,
    invocation: Invocation,
    cancellation: StreamCancellation,
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
    invocation: Invocation,
    cancellation: StreamCancellation,
    receiver: Receiver<Result<Completion, ChatError>>,
    ready: Option<Result<Completion, ChatError>>,
    completed: Arc<AtomicBool>,
    tool_metrics: ToolExecutionMetrics,
    steering: Option<crate::SteeringInbox>,
}

impl ChatJob {
    pub fn invocation(&self) -> &Invocation {
        &self.invocation
    }

    pub fn tool_metrics(&self) -> &ToolExecutionMetrics {
        &self.tool_metrics
    }

    /// Keep a handle to recover input not delivered before cancellation or failure.
    pub fn steering(&self) -> Option<&crate::SteeringInbox> {
        self.steering.as_ref()
    }

    pub fn steer(&self, text: impl Into<String>) -> Result<(), ChatError> {
        self.steering
            .as_ref()
            .context(SteeringClosedSnafu)?
            .submit(text.into())
    }

    pub fn cancel(&self) {
        if let Some(inbox) = &self.steering {
            inbox.close();
        }
        if self.completed.load(Ordering::Acquire) {
            return;
        }
        self.cancellation.cancel(
            mf_runtime::StreamExecutionSnafu {
                message: "chat request cancelled",
            }
            .build(),
        );
    }

    pub fn poll(&mut self) -> bool {
        if self.ready.is_none() {
            match self.receiver.try_recv() {
                Ok(result) => self.ready = Some(result),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.ready = Some(
                        WorkerPanicSnafu {
                            id: &self.invocation.call_id,
                        }
                        .fail(),
                    )
                }
            }
        }
        self.ready.is_some()
    }

    pub fn join(mut self) -> Result<Completion, ChatError> {
        if let Some(result) = self.ready.take() {
            return result;
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
        let invocation = Invocation {
            session_id: session_id.into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            call_id: uuid::Uuid::new_v4().to_string(),
        };
        let cancellation = StreamCancellation::default();
        let (sender, receiver) = mpsc::channel();
        let mut workers = self
            .inner
            .workers
            .lock()
            .expect("worker state was not poisoned");
        ensure!(!workers.closed, ClosedSnafu);
        ensure!(
            !workers
                .active
                .values()
                .any(|active| active.invocation.session_id == session_id),
            SessionBusySnafu { id: session_id }
        );
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
        let steering = profile
            .backend()
            .steering_input()
            .is_some()
            .then(|| crate::SteeringInbox::new(invocation.clone()));
        workers.active.insert(
            invocation.call_id.clone(),
            Active {
                steering: steering.clone(),
                invocation: invocation.clone(),
                cancellation: cancellation.clone(),
            },
        );
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
        let completed = Arc::new(AtomicBool::new(false));
        let worker_completed = completed.clone();
        let job_invocation = invocation.clone();
        let job_cancellation = cancellation.clone();
        let worker_cancellation = job_cancellation.clone();
        let tool_metrics = ToolExecutionMetrics::default();
        let worker_metrics = tool_metrics.clone();
        let worker_steering = steering.clone();
        let worker = thread::Builder::new()
            .name(format!("mf-chat-{}", invocation.call_id))
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    inner.conversation(
                        request,
                        profile,
                        job_cancellation,
                        ConversationPresentation {
                            preview,
                            steering: worker_steering.clone(),
                            tools: Some(worker_metrics.clone()),
                            context_windows: inner.context_windows.clone(),
                        },
                    )
                }));
                if let Some(inbox) = worker_steering {
                    inbox.close();
                }
                worker_metrics.close(worker_cancellation.failure().is_some());
                let result = match result {
                    Ok(result) => result,
                    Err(_) => {
                        let original = WorkerPanicSnafu {
                            id: &job_invocation.call_id,
                        }
                        .build();
                        let cleanup = inner
                            .database
                            .fail_turn(job_invocation.clone(), "failed", "chat worker panicked")
                            .err()
                            .map(Box::new);
                        Err(ExecutionSnafu {
                            invocation: job_invocation.clone(),
                            cleanup,
                        }
                        .into_error(Box::new(original)))
                    }
                };
                let result = match result {
                    Err(source)
                        if !matches!(
                            &source,
                            ChatError::Execution { .. } | ChatError::Uncommitted { .. }
                        ) =>
                    {
                        Err(ExecutionSnafu {
                            invocation: job_invocation.clone(),
                            cleanup: None,
                        }
                        .into_error(Box::new(source)))
                    }
                    result => result,
                };
                inner
                    .workers
                    .lock()
                    .expect("worker state was not poisoned")
                    .active
                    .remove(&job_invocation.call_id);
                worker_completed.store(true, Ordering::Release);
                let _ = sender.send(result);
            });
        match worker {
            Ok(worker) => workers.handles.push(worker),
            Err(source) => {
                workers.active.remove(&invocation.call_id);
                return Err(WorkerSpawnSnafu.into_error(source));
            }
        }
        Ok(ChatJob {
            invocation,
            cancellation,
            receiver,
            ready: None,
            completed,
            steering,
            tool_metrics,
        })
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
                if let Some(inbox) = &active.steering {
                    inbox.close();
                }
                active.cancellation.cancel(
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
    fn conversation(
        &self,
        request: BeginTurnInputs,
        profile: crate::WorkflowProfile,
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
