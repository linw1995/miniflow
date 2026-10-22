use crate::{
    ChatDatabase, ChatError, Completion, Invocation, ModelIdentity, UncommittedCompletion,
    error::*,
    flows,
    nodes::{BeginTurnInputs, RecoveryInputs, RecoveryOutputs},
};
use mf_runtime::{
    ExecutionContext, FlowRuntime, StreamCancellation, StreamError, WorkflowArguments,
};
use mf_telemetry::{identity::RunId, observation::Observer};
use serde_json::Value;
use snafu::{IntoError, OptionExt, ResultExt, ensure};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
    thread::{self, JoinHandle},
};

extern crate mfn_code as _;
extern crate mfn_core as _;

#[derive(Clone, Debug)]
pub struct ModelProfile {
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
        let identity = ModelIdentity {
            profile: name,
            model: Some(model),
        };
        Ok(Self { config, identity })
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
            profiles: BTreeMap::from([(profile.identity.profile.clone(), profile)]),
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
}

impl ChatJob {
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
                reason: "must contain a model profile"
            }
        );
        for (key, profile) in &config.profiles {
            ensure!(
                key == &profile.identity.profile,
                InvalidSnafu {
                    field: "profiles",
                    reason: "keys must match profile names"
                }
            );
        }
        if let Some(key) = &config.title_profile {
            ensure!(
                config.profiles.contains_key(key),
                ProfileMissingSnafu { name: key }
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
        self.start(session_id, prompt, None)
    }

    pub fn retry(&self, turn_id: &str) -> Result<ChatJob, ChatError> {
        let (original, prompt) = self.database().retry_prompt(turn_id)?;
        self.start(&original.session_id, &prompt, Some(turn_id.into()))
    }

    fn start(
        &self,
        session_id: &str,
        prompt: &str,
        retry_of: Option<String>,
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
                profile: profile.identity.profile.clone(),
                model: None,
            },
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
        profile: ModelProfile,
        cancellation: StreamCancellation,
    ) -> Result<Completion, ChatError> {
        let invocation = Invocation {
            session_id: request.session_id.clone(),
            turn_id: request.turn_id.clone(),
            call_id: request.call_id.clone(),
        };
        let mut plan = flows::conversation::plan()?;
        let title_profile = self
            .config
            .title_profile
            .as_ref()
            .and_then(|name| self.config.profiles.get(name))
            .unwrap_or(&profile);
        for (id, profile) in [("model", &profile), ("title_model", title_profile)] {
            let model = plan
                .definition
                .nodes
                .iter_mut()
                .find(|node| node.id.as_str() == id)
                .context(InvalidSnafu {
                    field: "conversation flow",
                    reason: "requires its model nodes",
                })?;
            let mut attribution = model.config["attribution"].clone();
            attribution["profile"] = profile.identity.profile.clone().into();
            model
                .config
                .as_object_mut()
                .expect("embedded model configuration is an object")
                .extend(
                    profile
                        .config
                        .as_object()
                        .expect("validated model profile")
                        .clone(),
                );
            model.config["attribution"] = attribution;
        }
        let registry = mf_runtime::NodeRegistry::from_inventory()?;
        let contract = crate::contract::ChatContract::from_manifest(&plan.manifest(&registry)?)?;
        let observation = self
            .config
            .observer
            .as_ref()
            .map(|observer| plan.start_observation(observer, RunId::new()))
            .transpose()?;
        let mut begun = crate::turn::begin(request)?;
        if let Some(context) = &mut begun.request.context {
            context.title.enabled = self.config.automatic_titles;
        }
        let result = ExecutionContext::run(observation, |context| -> Result<_, ChatError> {
            context.set_cancellation(cancellation.clone());
            context.set_workflow_arguments(
                contract.arguments(begun.request, &WorkflowArguments::default())?,
            );
            let flow = mf_compiler::instantiate_compiled_with_observation(
                &plan,
                &registry,
                context.observation_mut(),
            )?;
            Ok(FlowRuntime::default().execute_in_context(&flow, context)?)
        });
        let outputs = match result {
            Ok(outputs) => outputs,
            Err(source) => {
                let state = if cancellation.failure().is_some() {
                    "cancelled"
                } else {
                    "failed"
                };
                let cleanup = self
                    .database
                    .fail_turn(invocation.clone(), state, &source.to_string())
                    .err()
                    .map(Box::new);
                return Err(ExecutionSnafu {
                    invocation,
                    cleanup,
                }
                .into_error(Box::new(source)));
            }
        };
        let resolution = self.database.reconcile_result(
            &invocation,
            Some(crate::ConversationResult::decode(outputs)),
            crate::contract::TurnExit::Failed,
            "conversation workflow returned an invalid result",
        )?;
        match resolution.state {
            crate::contract::CompletionState::Uncommitted(pending) => {
                self.database.save_recovered(pending)
            }
            crate::contract::CompletionState::Committed(completion) => Ok(completion),
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
