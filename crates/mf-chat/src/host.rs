use crate::{
    CallPurpose, ChatDatabase, ChatError, Completion, Invocation, ModelIdentity, TitleCompletion,
    TitleOutcome,
    error::*,
    flows,
    nodes::{
        BeginTurnInputs, Changed, CompletionOutputs, FailTurnInputs, RecoveryInputs,
        RecoveryOutputs,
    },
    operation, turn,
};
use mf_runtime::{
    ContextValue, ExecutionContext, FlowRuntime, NodeValue, StreamCancellation, StreamError,
    WorkflowArguments,
};
use mf_telemetry::{identity::RunId, observation::Observer};
use serde_json::Value;
use sha2::{Digest, Sha256};
use snafu::{IntoError, OptionExt, ResultExt, ensure};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread::{self, JoinHandle},
};

mod title;

#[derive(Clone, Debug)]
pub struct ModelProfile {
    name: String,
    config: Value,
    identity: ModelIdentity,
}

impl ModelProfile {
    pub fn new(name: impl Into<String>, config: Value) -> Result<Self, ChatError> {
        let name = name.into();
        ensure!(
            !name.trim().is_empty(),
            InvalidSnafu {
                field: "profile",
                reason: "must be nonblank"
            }
        );
        mfn_llm::prepare_llm(config.clone()).context(ModelBuildSnafu)?;
        let model = config
            .get("model")
            .and_then(Value::as_str)
            .context(InvalidSnafu {
                field: "model",
                reason: "must be a string",
            })?
            .to_owned();
        let encoded = serde_json::to_vec(&config).context(JsonSnafu)?;
        let identity = ModelIdentity {
            profile: name.clone(),
            model,
            config_id: Sha256::digest(encoded)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        };
        Ok(Self {
            name,
            config,
            identity,
        })
    }

    pub fn identity(&self) -> &ModelIdentity {
        &self.identity
    }
}

#[derive(Clone)]
pub struct HostConfig {
    pub profiles: BTreeMap<String, ModelProfile>,
    pub max_active: usize,
    pub max_history_bytes: usize,
    pub observer: Option<Observer>,
    pub title_profile: Option<String>,
    pub automatic_titles: bool,
}

impl HostConfig {
    pub fn new(profile: ModelProfile) -> Self {
        Self {
            profiles: BTreeMap::from([(profile.name.clone(), profile)]),
            max_active: 4,
            max_history_bytes: crate::DEFAULT_HISTORY_BYTES,
            observer: None,
            title_profile: None,
            automatic_titles: true,
        }
    }
}

struct Active {
    invocation: Invocation,
    cancellation: StreamCancellation,
    purpose: CallPurpose,
}

#[derive(Default)]
struct Workers {
    closed: bool,
    active: BTreeMap<String, Active>,
    handles: Vec<JoinHandle<()>>,
    background: VecDeque<TitleJob>,
    background_reserved: usize,
}

struct Inner {
    database: ChatDatabase,
    config: HostConfig,
    workers: Mutex<Workers>,
    foreground_admissions: AtomicUsize,
}

struct ForegroundAdmission<'a>(&'a AtomicUsize);

impl<'a> ForegroundAdmission<'a> {
    fn enter(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self(counter)
    }
}

impl Drop for ForegroundAdmission<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct ChatHost {
    inner: Arc<Inner>,
}

pub struct ChatJob<T = Completion> {
    invocation: Invocation,
    cancellation: StreamCancellation,
    receiver: Receiver<Result<T, ChatError>>,
    ready: Option<Result<T, ChatError>>,
}

pub type TitleJob = ChatJob<TitleCompletion>;

#[derive(Clone, Debug)]
pub struct UncommittedCompletion {
    pub database_path: PathBuf,
    pub completion: Completion,
    pub purpose: CallPurpose,
}

impl<T> ChatJob<T> {
    pub fn invocation(&self) -> &Invocation {
        &self.invocation
    }

    pub fn cancel(&self) {
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

    pub fn join(mut self) -> Result<T, ChatError> {
        if let Some(result) = self.ready.take() {
            return result;
        }
        self.receiver.recv().context(WorkerReceiveSnafu)?
    }
}

impl<T> Drop for ChatJob<T> {
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
                reason: "must contain a model profile"
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
                foreground_admissions: AtomicUsize::new(0),
            }),
        };
        if let Some(key) = &host.inner.config.title_profile {
            ensure!(
                host.inner.config.profiles.contains_key(key),
                ProfileMissingSnafu { name: key }
            );
        }
        if host.inner.config.automatic_titles {
            host.dispatch_titles()?;
        }
        Ok(host)
    }

    pub fn database(&self) -> &ChatDatabase {
        &self.inner.database
    }

    pub fn submit(&self, session_id: &str, prompt: &str) -> Result<ChatJob, ChatError> {
        self.start(session_id, prompt, None)
    }

    pub fn retry(&self, turn_id: &str) -> Result<ChatJob, ChatError> {
        let original: turn::TurnRequestOutputs = operation::execute(
            self.database().path(),
            "mf_chat.turn.request",
            turn::RecordInputs {
                database_path: String::new(),
                id: turn_id.into(),
            },
        )?;
        ensure!(
            matches!(
                original.turn.state.as_str(),
                "failed" | "cancelled" | "interrupted"
            ),
            InvalidSnafu {
                field: "retry_of",
                reason: "must identify an unsuccessful turn",
            }
        );
        self.start(
            &original.turn.session_id,
            &original.prompt,
            Some(turn_id.into()),
        )
    }

    fn start(
        &self,
        session_id: &str,
        prompt: &str,
        retry_of: Option<String>,
    ) -> Result<ChatJob, ChatError> {
        let _admission = ForegroundAdmission::enter(&self.inner.foreground_admissions);
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
                .any(|active| active.purpose == CallPurpose::Conversation
                    && active.invocation.session_id == session_id),
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
                purpose: CallPurpose::Conversation,
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
            model: profile.identity.clone(),
            retry_of,
            max_history_bytes: self.inner.config.max_history_bytes,
        };
        let job_invocation = invocation.clone();
        let job_cancellation = cancellation.clone();
        let worker = thread::Builder::new()
            .name(format!("mf-chat-{}", invocation.call_id))
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    inner.conversation(request, profile, job_cancellation)
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
                let successful = result.is_ok();
                let _ = sender.send(result);
                if successful && inner.config.automatic_titles {
                    let _ = inner.dispatch_titles();
                }
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
        })
    }

    pub fn retry_completion(
        &self,
        pending: UncommittedCompletion,
    ) -> Result<Completion, ChatError> {
        ensure!(
            pending.purpose == CallPurpose::Conversation,
            InvalidSnafu {
                field: "purpose",
                reason: "use retry_title_completion for a title response",
            }
        );
        ensure!(
            pending.database_path == self.database().path(),
            InvalidSnafu {
                field: "database_path",
                reason: "completion belongs to another host database"
            }
        );
        ensure!(
            !self
                .inner
                .workers
                .lock()
                .expect("worker state was not poisoned")
                .closed,
            ClosedSnafu
        );
        match self.database().complete_turn(
            pending.completion.invocation.clone(),
            pending.completion.reply.clone(),
        ) {
            Ok(completion) => {
                if self.inner.config.automatic_titles {
                    let _ = self.inner.dispatch_titles();
                }
                Ok(completion)
            }
            Err(source) => Err(UncommittedSnafu {
                pending: Box::new(pending),
            }
            .into_error(Box::new(source))),
        }
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
        let _: RecoveryOutputs = operation::execute(
            self.database().path(),
            "mf_chat.storage.recover",
            RecoveryInputs {
                database_path: String::new(),
            },
        )?;
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
        profile: ModelProfile,
        cancellation: StreamCancellation,
    ) -> Result<Completion, ChatError> {
        let invocation = Invocation {
            session_id: request.session_id.clone(),
            turn_id: request.turn_id.clone(),
            call_id: request.call_id.clone(),
        };
        let mut plan = flows::conversation::plan()?;
        let model = plan
            .definition
            .nodes
            .iter_mut()
            .find(|node| node.id.as_str() == "model")
            .context(InvalidSnafu {
                field: "conversation flow",
                reason: "requires its model node",
            })?;
        model.config = profile.config;
        let observation = self
            .config
            .observer
            .as_ref()
            .map(|observer| plan.start_observation(observer, RunId::new()))
            .transpose()?;
        ExecutionContext::run(observation, |context| -> Result<Completion, ChatError> {
            context.set_cancellation(cancellation.clone());
            context.set_workflow_arguments(WorkflowArguments(
                request
                    .into_values()?
                    .into_iter()
                    .map(|(name, value)| (format!("begin.{name}"), value))
                    .collect(),
            ));
            let registry = mf_runtime::NodeRegistry::from_inventory()?;
            let flow = mf_compiler::instantiate_compiled_with_observation(
                &plan,
                &registry,
                context.observation_mut(),
            )?;
            let result = FlowRuntime::default().execute_in_context(&flow, context);
            match result {
                Ok(outputs) => Ok(CompletionOutputs::from_values(outputs)?.completion),
                Err(source) => {
                    let begun = matches!(
                        context.output("begin.invocation"),
                        Ok(ContextValue::Value(_))
                    );
                    if begun
                        && let Ok(ContextValue::Value(response)) = context.output("model.response")
                    {
                        let reply =
                            crate::model::ModelResponse::from_values(mf_runtime::Inputs::from([
                                ("response".into(), response.clone()),
                            ]))?
                            .response;
                        return Err(UncommittedSnafu {
                            pending: Box::new(UncommittedCompletion {
                                purpose: CallPurpose::Conversation,
                                database_path: self.database.path().to_owned(),
                                completion: Completion { invocation, reply },
                            }),
                        }
                        .into_error(Box::new(ChatError::from(source))));
                    }
                    let cleanup = if begun {
                        let state = if cancellation.failure().is_some() {
                            "cancelled"
                        } else {
                            "failed"
                        };
                        operation::execute::<_, Changed>(
                            self.database.path(),
                            "mf_chat.turn.fail",
                            FailTurnInputs {
                                database_path: String::new(),
                                invocation: invocation.clone(),
                                state: state.into(),
                                error: source.to_string(),
                            },
                        )
                        .err()
                        .map(Box::new)
                    } else {
                        None
                    };
                    Err(ExecutionSnafu {
                        invocation,
                        cleanup,
                    }
                    .into_error(Box::new(ChatError::from(source))))
                }
            }
        })
    }
}
