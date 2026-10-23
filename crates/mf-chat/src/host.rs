use crate::{
    ChatDatabase, ChatError, Completion, Invocation, ModelIdentity, UncommittedCompletion,
    error::*,
    nodes::{BeginTurnInputs, RecoveryInputs, RecoveryOutputs},
};
use mf_runtime::{StreamCancellation, StreamError};
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
    pub max_history_bytes: usize,
    pub observer: Option<Observer>,
    pub automatic_titles: bool,
}

impl HostConfig {
    pub fn new(profile: crate::WorkflowProfile) -> Self {
        Self {
            profiles: BTreeMap::from([(profile.name.clone(), profile)]),
            max_active: 4,
            max_history_bytes: crate::DEFAULT_HISTORY_BYTES,
            observer: None,
            automatic_titles: true,
        }
    }
}

struct Active {
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
}

impl ChatJob {
    pub fn invocation(&self) -> &Invocation {
        &self.invocation
    }

    pub fn cancel(&self) {
        if self.completed.load(Ordering::Acquire) {
            return;
        }
        self.cancellation.cancel(StreamError::Execution {
            message: "chat request cancelled".into(),
        });
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
            config.max_active > 0 && config.max_history_bytes > 0,
            InvalidSnafu {
                field: "host limits",
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

    pub fn submit_stream(
        &self,
        session_id: &str,
        prompt: &str,
        limits: mf_runtime::LiveLimits,
    ) -> Result<(ChatJob, mf_runtime::LiveReceiver), ChatError> {
        let (publisher, receiver) = mf_runtime::live_channel(limits)?;
        let job = self.start(
            session_id,
            prompt,
            None,
            Some(publisher.select(["answer".into()])),
        )?;
        Ok((job, receiver))
    }

    pub fn retry_stream(
        &self,
        turn_id: &str,
        limits: mf_runtime::LiveLimits,
    ) -> Result<(ChatJob, mf_runtime::LiveReceiver), ChatError> {
        let (original, prompt) = self.database().retry_prompt(turn_id)?;
        let (publisher, receiver) = mf_runtime::live_channel(limits)?;
        let job = self.start(
            &original.session_id,
            &prompt,
            Some(turn_id.into()),
            Some(publisher.select(["answer".into()])),
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
        live: Option<mf_runtime::LivePublisher>,
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
        workers.active.insert(
            invocation.call_id.clone(),
            Active {
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
            max_history_bytes: self.inner.config.max_history_bytes,
        };
        let completed = Arc::new(AtomicBool::new(false));
        let worker_completed = completed.clone();
        let job_invocation = invocation.clone();
        let job_cancellation = cancellation.clone();
        let worker = thread::Builder::new()
            .name(format!("mf-chat-{}", invocation.call_id))
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    inner.conversation(request, profile, job_cancellation, live)
                }));
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
                active.cancellation.cancel(StreamError::Execution {
                    message: "chat host closed".into(),
                });
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
        live: Option<mf_runtime::LivePublisher>,
    ) -> Result<Completion, ChatError> {
        let invocation = Invocation {
            session_id: request.session_id.clone(),
            turn_id: request.turn_id.clone(),
            call_id: request.call_id.clone(),
        };
        let resolution = crate::workflow::run_conversation_with_live(
            &self.database,
            request,
            profile.backend(),
            cancellation,
            self.config.automatic_titles,
            self.config.observer.as_ref(),
            live,
        )?;
        match resolution.state {
            crate::contract::CompletionState::Committed(completion) => Ok(completion),
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
