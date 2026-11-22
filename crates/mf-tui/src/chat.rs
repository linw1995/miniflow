//! A single conversation execution lane over a supplied workflow executable.

use crate::{
    run::{LocalObservation, LocalOutcome, RunError, RunOptions},
    runner::{PreparedRunner, RunnerError, RunnerInvocation},
};
use mf_chat::contract::{
    ChatContract, CompletionState, ConversationExit, ProviderParameterPorts, ReconciledCompletion,
    TurnExit,
};
use mf_chat::{ChatDatabase, Invocation, MessagePage, Session, SessionPage, UncommittedCompletion};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

mod preferences;
mod preview;
pub use preview::LivePreview;
mod transcript;
mod ui;
pub use ui::run_chat;
const QUEUE_CAPACITY: usize = 16;
const PAGE_SIZE: usize = 50;
const MODEL_LIST_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum ChatRunError {
    #[snafu(transparent)]
    Json { source: serde_json::Error },
    #[snafu(transparent)]
    Domain { source: mf_chat::ChatError },
    #[snafu(transparent)]
    Inputs {
        source: mf_runtime::WorkflowInputError,
    },
    #[snafu(transparent)]
    Observation { source: RunError },
    #[snafu(transparent)]
    Runner { source: RunnerError },
    #[snafu(transparent)]
    NodeRegistry {
        source: mf_runtime::NodeRegistryError,
    },
    #[snafu(transparent)]
    NodeBuild { source: mf_runtime::NodeBuildError },
    #[snafu(display("could not {operation}: {source}"))]
    Io {
        operation: &'static str,
        source: io::Error,
    },
    #[snafu(display("chat requires terminal stdin and stderr"))]
    TerminalRequired,
    #[snafu(display("turn cancelled before launch"))]
    Cancelled,
    #[snafu(display("chat command queue is full"))]
    QueueFull,
    #[snafu(display("chat controller is closed"))]
    Closed,
    #[snafu(display("chat controller worker panicked"))]
    WorkerPanic,
    #[snafu(display("finish, cancel, or recover the current turn first"))]
    Busy,
    #[snafu(display("there is no reply to recover"))]
    MissingReply,
    #[snafu(display("invalid chat options: {reason}"))]
    Options { reason: &'static str },
    #[snafu(display("invalid chat preferences {path:?}: {reason}"))]
    Preferences { path: PathBuf, reason: String },
    #[snafu(display("invalid chat preferences JSON in {path:?}: {source}"))]
    PreferencesJson {
        path: PathBuf,
        source: serde_json::Error,
    },
}

#[derive(Clone, Debug)]
pub struct ChatOptions {
    pub database_path: PathBuf,
    pub profile: String,
    pub run: RunOptions,
}

pub struct ChatLaunch {
    pub runner: PreparedRunner,
    pub contract: ChatContract,
    pub options: ChatOptions,
    pub parameters: mf_runtime::WorkflowArguments,
}
impl ChatLaunch {
    pub fn prepare(executable: &Path, options: ChatOptions) -> Result<Self, ChatRunError> {
        ensure!(
            !options.profile.trim().is_empty(),
            OptionsSnafu {
                reason: "requires a nonblank profile"
            }
        );
        let runner =
            PreparedRunner::load(executable, mf_chat::contract::MAX_RECOVERY_BYTES as u64)?;
        let contract = ChatContract::from_manifest(runner.manifest())?;
        let parameters = options
            .run
            .parsed_workflow_arguments(&contract.parameters)?;
        let provider_ports = contract.provider_parameter_ports();
        let prefs_path = preferences::preferences_path(&options.database_path);
        preferences::resolve_launch_preferences(
            &contract.parameters,
            provider_ports.as_ref(),
            &prefs_path,
            &options.profile,
            &parameters,
        )?;
        Ok(Self {
            runner,
            contract,
            options,
            parameters,
        })
    }
}

pub enum ChatCommand {
    ListSessions {
        after: Option<String>,
    },
    Load {
        session_id: String,
        before: Option<i64>,
    },
    ListModels,
    SelectModel {
        model: String,
    },
    Submit {
        session_id: Option<String>,
        prompt: String,
    },
    Retry {
        turn_id: String,
    },
    Save,
    Abandon,
}

pub enum ChatEvent {
    Sessions(SessionPage),
    Loaded {
        session: Session,
        page: MessagePage,
        context_tokens: Option<u64>,
    },
    Models(Result<Vec<mfn_llm::ModelInfo>, String>),
    ModelSelected {
        model: Option<String>,
        context_window: Option<u64>,
        automatic: bool,
        warning: Option<String>,
    },
    ContextWindow {
        model: String,
        context_window: Option<u64>,
    },
    Submitted {
        session: Session,
        invocation: Invocation,
        prompt: String,
    },
    Observing(LocalObservation),
    Preview {
        invocation: Invocation,
        preview: LivePreview,
    },
    Finished {
        invocation: Invocation,
        resolution: Result<ReconciledCompletion, mf_chat::ChatError>,
        status: Option<ExitStatus>,
        stderr: String,
        capture_error: Option<String>,
    },
    Error {
        admission: bool,
        source: ChatRunError,
    },
}

pub struct ChatController {
    _database: Arc<ChatDatabase>,
    commands: SyncSender<ChatCommand>,
    events: Receiver<ChatEvent>,
    deferred: Mutex<Option<ChatEvent>>,
    preview: Arc<Mutex<LivePreview>>,
    preview_revision: AtomicU64,
    presented_invocation: Mutex<Option<Invocation>>,
    supports_model_selection: bool,
    initial_model: Option<String>,
    loading_default_model: bool,
    model_query_cancel: Arc<Mutex<Option<mf_runtime::StreamCancellation>>>,
    closed: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<(), ChatRunError>>>,
}
impl ChatController {
    pub fn open(launch: ChatLaunch) -> Result<Self, ChatRunError> {
        let database = Arc::new(ChatDatabase::open(&launch.options.database_path)?);
        let provider_ports = launch.contract.provider_parameter_ports();
        let prefs_path = preferences::preferences_path(database.path());
        let merged = preferences::resolve_launch_preferences(
            &launch.contract.parameters,
            provider_ports.as_ref(),
            &prefs_path,
            &launch.options.profile,
            &launch.parameters,
        )?;
        if merged.preferences_changed {
            preferences::persist_profile(
                &prefs_path,
                &launch.options.profile,
                merged.updated_profile,
            )?;
        }
        let supports_model_selection = provider_ports.is_some();
        let list_models_node = supports_model_selection
            .then(prepare_list_models_node)
            .transpose()?;
        let model_info_node = supports_model_selection
            .then(prepare_model_info_node)
            .transpose()?;
        let loading_default_model = merged.needs_default_model;
        let initial_model = if loading_default_model {
            None
        } else {
            provider_ports.as_ref().and_then(|ports| {
                preferences::resolve_display_model(
                    &launch.contract.parameters,
                    ports,
                    &merged.effective,
                    |name| std::env::var_os(name),
                )
            })
        };
        let (commands, input) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (output, events) = mpsc::sync_channel(QUEUE_CAPACITY);
        let closed = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let preview = Arc::new(Mutex::new(LivePreview::default()));
        let model_query_cancel = Arc::new(Mutex::new(None));
        let controller = Controller {
            launch: Arc::new(launch),
            parameters: merged.effective,
            provider_ports,
            list_models_node,
            model_info_node,
            model_context_windows: BTreeMap::new(),
            needs_default_model: loading_default_model,
            model_query_cancel: Arc::clone(&model_query_cancel),
            database: Arc::clone(&database),
            input,
            output,
            closed: Arc::clone(&closed),
            busy: Arc::clone(&busy),
            cancel: Arc::clone(&cancel),
            retained: None,
            preview: preview.clone(),
        };
        let worker = thread::Builder::new()
            .name("mf-chat-controller".into())
            .spawn(move || controller.run())
            .context(IoSnafu {
                operation: "start chat controller",
            })?;
        Ok(Self {
            _database: database,
            commands,
            events,
            deferred: Mutex::new(None),
            preview,
            preview_revision: AtomicU64::new(0),
            presented_invocation: Mutex::new(None),
            supports_model_selection,
            initial_model,
            loading_default_model,
            model_query_cancel,
            closed,
            busy,
            cancel,
            worker: Some(worker),
        })
    }
    pub fn supports_model_selection(&self) -> bool {
        self.supports_model_selection
    }
    pub fn initial_model(&self) -> Option<&str> {
        self.initial_model.as_deref()
    }
    pub fn loading_default_model(&self) -> bool {
        self.loading_default_model
    }
    pub fn send(&self, command: ChatCommand) -> Result<(), ChatRunError> {
        ensure!(!self.closed.load(Ordering::Acquire), ClosedSnafu);
        let admission = matches!(
            command,
            ChatCommand::Submit { .. } | ChatCommand::Retry { .. }
        );
        if admission {
            ensure!(
                self.busy
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok(),
                BusySnafu
            );
            self.cancel.store(false, Ordering::Release);
        }
        let result = match self.commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => QueueFullSnafu.fail(),
            Err(TrySendError::Disconnected(_)) => ClosedSnafu.fail(),
        };
        if result.is_err() && admission {
            self.busy.store(false, Ordering::Release);
        }
        result
    }
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        self.cancel_model_query("chat request cancelled");
    }
    fn cancel_model_query(&self, message: &str) {
        if let Some(cancellation) = self.model_query_cancel.lock().unwrap().as_ref() {
            cancellation.cancel(mf_runtime::StreamExecutionSnafu { message }.build());
        }
    }
    pub fn try_event(&self) -> Option<ChatEvent> {
        let event = self
            .deferred
            .lock()
            .unwrap()
            .take()
            .or_else(|| self.events.try_recv().ok());
        if let Some(ChatEvent::Submitted { invocation, .. }) = &event {
            *self.presented_invocation.lock().unwrap() = Some(invocation.clone());
        }
        if matches!(event, Some(ChatEvent::Finished { .. }))
            && let Some(preview) = self.updated_preview()
        {
            *self.deferred.lock().unwrap() = event;
            return Some(preview);
        }
        if matches!(event, Some(ChatEvent::Finished { .. })) {
            *self.presented_invocation.lock().unwrap() = None;
        }
        event.or_else(|| self.updated_preview())
    }

    fn updated_preview(&self) -> Option<ChatEvent> {
        let expected = self.presented_invocation.lock().unwrap().clone();
        let preview = self.preview.lock().unwrap();
        if expected.is_none() || preview.invocation != expected {
            return None;
        }
        if self.preview_revision.load(Ordering::Acquire) == preview.revision {
            return None;
        }
        self.preview_revision
            .store(preview.revision, Ordering::Release);
        if preview.segments.is_empty() && !preview.omitted {
            return None;
        }
        preview
            .invocation
            .as_ref()
            .map(|invocation| ChatEvent::Preview {
                invocation: invocation.clone(),
                preview: preview.clone(),
            })
    }
    pub fn close(mut self) -> Result<(), ChatRunError> {
        self.join_worker()
    }
    fn join_worker(&mut self) -> Result<(), ChatRunError> {
        self.closed.store(true, Ordering::Release);
        self.cancel_model_query("chat controller closed");
        if let Some(worker) = self.worker.take() {
            match worker.join() {
                Ok(result) => result?,
                Err(_) => return WorkerPanicSnafu.fail(),
            }
        }
        Ok(())
    }
}
impl Drop for ChatController {
    fn drop(&mut self) {
        let _ = self.join_worker();
    }
}

enum RetainedResponse {
    Ready(UncommittedCompletion),
    Exited(LocalOutcome),
}
struct Retained {
    invocation: Invocation,
    response: RetainedResponse,
}
struct Controller {
    launch: Arc<ChatLaunch>,
    parameters: mf_runtime::WorkflowArguments,
    provider_ports: Option<ProviderParameterPorts>,
    list_models_node: Option<mf_runtime::PreparedNode>,
    model_info_node: Option<mf_runtime::PreparedNode>,
    model_context_windows: BTreeMap<(String, String), Option<u64>>,
    needs_default_model: bool,
    model_query_cancel: Arc<Mutex<Option<mf_runtime::StreamCancellation>>>,
    database: Arc<ChatDatabase>,
    input: Receiver<ChatCommand>,
    output: SyncSender<ChatEvent>,
    closed: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    retained: Option<Retained>,
    preview: Arc<Mutex<LivePreview>>,
}

fn prepare_list_models_node() -> Result<mf_runtime::PreparedNode, ChatRunError> {
    let registry = mf_runtime::NodeRegistry::from_inventory()?;
    let factory = registry
        .get(mfn_llm::LIST_MODELS_KIND)
        .context(OptionsSnafu {
            reason: "llm.list_models node is not registered",
        })?;
    Ok(factory.instantiate(serde_json::json!({
        "base_url": {"$ref": "params.base_url"},
        "api_key": {"$ref": "params.api_key"},
        "timeout_ms": MODEL_LIST_TIMEOUT_MS,
    }))?)
}

fn prepare_model_info_node() -> Result<mf_runtime::PreparedNode, ChatRunError> {
    let registry = mf_runtime::NodeRegistry::from_inventory()?;
    let factory = registry
        .get(mfn_llm::MODEL_INFO_KIND)
        .context(OptionsSnafu {
            reason: "llm.model_info node is not registered",
        })?;
    Ok(factory.instantiate(serde_json::json!({
        "model": {"$ref": "params.model"},
        "base_url": {"$ref": "params.base_url"},
        "api_key": {"$ref": "params.api_key"},
        "timeout_ms": MODEL_LIST_TIMEOUT_MS,
    }))?)
}

fn model_list_context(
    endpoint: preferences::ModelListEndpoint,
    cancellation: mf_runtime::StreamCancellation,
) -> Result<mf_runtime::ExecutionContext, mf_runtime::NodeExecutionError> {
    let source: mf_runtime::FlowNode<()> = mf_runtime::FlowNode {
        definition_id: "params".into(),
        node: (),
        metadata: mf_runtime::NodeMetadata {
            ports: mf_runtime::NodePorts {
                inputs: Vec::new(),
                outputs: vec![
                    mf_runtime::PortSpec::new("base_url", mf_runtime::ValueType::String, true),
                    mf_runtime::PortSpec::new("api_key", mf_runtime::ValueType::String, true),
                ],
            },
            ..Default::default()
        },
        output_keys: std::sync::OnceLock::new(),
    };
    let mut context = mf_runtime::ExecutionContext::for_message(
        &source,
        mf_runtime::NodeResult {
            outputs: [
                (
                    "base_url".into(),
                    mf_runtime::ValueRef::from(endpoint.base_url),
                ),
                (
                    "api_key".into(),
                    mf_runtime::ValueRef::from(endpoint.api_key.unwrap_or_default()).into_secret(),
                ),
            ]
            .into(),
            ..Default::default()
        },
    )?;
    context.set_cancellation(cancellation);
    Ok(context)
}

fn model_info_context(
    model: &str,
    endpoint: preferences::ModelListEndpoint,
    cancellation: mf_runtime::StreamCancellation,
) -> Result<mf_runtime::ExecutionContext, mf_runtime::NodeExecutionError> {
    let source: mf_runtime::FlowNode<()> = mf_runtime::FlowNode {
        definition_id: "params".into(),
        node: (),
        metadata: mf_runtime::NodeMetadata {
            ports: mf_runtime::NodePorts {
                inputs: Vec::new(),
                outputs: vec![
                    mf_runtime::PortSpec::new("model", mf_runtime::ValueType::String, true),
                    mf_runtime::PortSpec::new("base_url", mf_runtime::ValueType::String, true),
                    mf_runtime::PortSpec::new("api_key", mf_runtime::ValueType::String, true),
                ],
            },
            ..Default::default()
        },
        output_keys: std::sync::OnceLock::new(),
    };
    let mut context = mf_runtime::ExecutionContext::for_message(
        &source,
        mf_runtime::NodeResult {
            outputs: [
                ("model".into(), mf_runtime::ValueRef::from(model)),
                (
                    "base_url".into(),
                    mf_runtime::ValueRef::from(endpoint.base_url),
                ),
                (
                    "api_key".into(),
                    mf_runtime::ValueRef::from(endpoint.api_key.unwrap_or_default()).into_secret(),
                ),
            ]
            .into(),
            ..Default::default()
        },
    )?;
    context.set_cancellation(cancellation);
    Ok(context)
}

fn positive_context_window(window: Option<i64>) -> Option<u64> {
    window
        .and_then(|value| u64::try_from(value).ok())
        .filter(|&value| value > 0)
}

impl Controller {
    fn emit(&self, event: ChatEvent) {
        emit(&self.output, &self.closed, event);
    }

    fn resolved_display_model(&self) -> Option<String> {
        self.provider_ports.as_ref().and_then(|ports| {
            preferences::resolve_display_model(
                &self.launch.contract.parameters,
                ports,
                &self.parameters,
                |name| std::env::var_os(name),
            )
        })
    }

    fn resolved_endpoint(&self) -> Option<preferences::ModelListEndpoint> {
        let ports = self.provider_ports.as_ref()?;
        preferences::resolve_model_list_endpoint(
            &self.launch.contract.parameters,
            ports,
            &self.parameters,
            |name| std::env::var_os(name),
        )
    }

    fn record_model_windows(&mut self, base_url: &str, models: &[mfn_llm::ModelInfo]) {
        let normalized = base_url.trim_end_matches('/');
        if normalized.is_empty() {
            return;
        }
        for model in models {
            self.model_context_windows.insert(
                (normalized.to_owned(), model.id.clone()),
                positive_context_window(model.context_window),
            );
        }
    }

    fn cached_context_window(&self, model: &str) -> Option<Option<u64>> {
        let endpoint = self.resolved_endpoint()?;
        let normalized = endpoint.base_url.trim_end_matches('/');
        if normalized.is_empty() {
            return None;
        }
        self.model_context_windows
            .get(&(normalized.to_owned(), model.to_owned()))
            .copied()
    }

    fn begin_model_query_cancellation(&self) -> mf_runtime::StreamCancellation {
        let cancellation = mf_runtime::StreamCancellation::default();
        let mut slot = self.model_query_cancel.lock().unwrap();
        *slot = Some(cancellation.clone());
        if self.closed.load(Ordering::Acquire) {
            cancellation.cancel(
                mf_runtime::StreamExecutionSnafu {
                    message: "chat controller closed",
                }
                .build(),
            );
        }
        cancellation
    }

    fn fetch_models(&mut self) -> Result<Vec<mfn_llm::ModelInfo>, String> {
        if self.provider_ports.is_none() || self.list_models_node.is_none() {
            return Err("workflow does not expose model selection parameters".into());
        }
        let Some(endpoint) = self.resolved_endpoint() else {
            return Err("provider base_url is not configured".into());
        };
        let base_url = endpoint.base_url.clone();
        let cancellation = self.begin_model_query_cancellation();
        let node = self
            .list_models_node
            .as_ref()
            .expect("list_models_node checked");
        let executed = model_list_context(endpoint, cancellation).and_then(|mut context| {
            let task = node.execution.as_task_node().expect("llm.list_models task");
            task.execute(mf_runtime::Inputs::new(), &mut context)
        });
        *self.model_query_cancel.lock().unwrap() = None;
        let executed = executed.map_err(|error| error.to_string())?;
        let outputs =
            <mfn_llm::ModelListOutputs as mf_runtime::NodeValue>::from_values(executed.outputs)
                .map_err(|error| error.to_string())?;
        self.record_model_windows(&base_url, &outputs.models);
        Ok(outputs.models)
    }

    fn probe_model_context_window(&mut self, model: &str) -> Option<u64> {
        if let Some(cached) = self.cached_context_window(model) {
            return cached;
        }
        let endpoint = self.resolved_endpoint()?;
        let normalized = endpoint.base_url.trim_end_matches('/').to_owned();
        let node = self.model_info_node.as_ref()?;
        let cancellation = self.begin_model_query_cancellation();
        let executed =
            model_info_context(model, endpoint, cancellation.clone()).and_then(|mut context| {
                let task = node.execution.as_task_node().expect("llm.model_info task");
                task.execute(mf_runtime::Inputs::new(), &mut context)
            });
        *self.model_query_cancel.lock().unwrap() = None;
        if cancellation.failure().is_some() || self.closed.load(Ordering::Acquire) {
            return None;
        }
        let discovered = executed
            .ok()
            .and_then(|result| {
                <mfn_llm::ModelInfoOutputs as mf_runtime::NodeValue>::from_values(result.outputs)
                    .ok()
            })
            .and_then(|outputs| positive_context_window(outputs.context_window));
        if !normalized.is_empty() {
            self.model_context_windows
                .insert((normalized, model.to_owned()), discovered);
        }
        discovered
    }

    fn discover_default_model(&mut self) {
        let fetched = self.fetch_models();
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        if let Ok(models) = fetched
            && let Some(first) = models.first()
            && let Some(ports) = &self.provider_ports
        {
            self.parameters.0.insert(
                ports.model.clone(),
                mf_runtime::ValueRef::from(first.id.as_str()),
            );
            let warning = preferences::persist_selected_model(
                &preferences::preferences_path(self.database.path()),
                &self.launch.options.profile,
                &self.launch.contract.parameters,
                &ports.model,
                &first.id,
            )
            .err()
            .map(|error| error.to_string());
            self.emit(ChatEvent::ModelSelected {
                model: Some(first.id.clone()),
                context_window: positive_context_window(first.context_window),
                automatic: true,
                warning,
            });
            return;
        }
        let fallback = self.resolved_display_model();
        let context_window = fallback
            .as_deref()
            .and_then(|model| self.cached_context_window(model).flatten());
        self.emit(ChatEvent::ModelSelected {
            model: fallback,
            context_window,
            automatic: true,
            warning: None,
        });
    }
}

fn emit(output: &SyncSender<ChatEvent>, closed: &AtomicBool, mut event: ChatEvent) {
    while !closed.load(Ordering::Acquire) {
        match output.try_send(event) {
            Ok(()) => return,
            Err(TrySendError::Full(pending)) => event = pending,
            Err(TrySendError::Disconnected(_)) => return,
        }
        thread::sleep(Duration::from_millis(10));
    }
}

impl Controller {
    fn run(mut self) -> Result<(), ChatRunError> {
        if self.needs_default_model && !self.closed.load(Ordering::Acquire) {
            self.discover_default_model();
        } else if !self.closed.load(Ordering::Acquire)
            && let Some(model) = self.resolved_display_model()
            && let Some(window) = self.probe_model_context_window(&model)
            && !self.closed.load(Ordering::Acquire)
        {
            self.emit(ChatEvent::ContextWindow {
                model,
                context_window: Some(window),
            });
        }
        while !self.closed.load(Ordering::Acquire) {
            match self.input.recv_timeout(Duration::from_millis(50)) {
                Ok(command) => {
                    if self.closed.load(Ordering::Acquire) {
                        break;
                    }
                    let admission = matches!(
                        command,
                        ChatCommand::Submit { .. } | ChatCommand::Retry { .. }
                    );
                    if let Err(source) = self.command(command) {
                        if self.closed.load(Ordering::Acquire) {
                            if matches!(source, ChatRunError::Cancelled) {
                                break;
                            }
                            return Err(source);
                        }
                        if admission {
                            self.busy.store(self.retained.is_some(), Ordering::Release);
                        }
                        self.emit(ChatEvent::Error { admission, source });
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if self.retained.is_some() {
            self.recover(true)?;
        }
        Ok(())
    }
    fn command(&mut self, command: ChatCommand) -> Result<(), ChatRunError> {
        match command {
            ChatCommand::ListSessions { after } => {
                self.emit(ChatEvent::Sessions(
                    self.database.list_sessions(false, PAGE_SIZE, after)?,
                ));
            }
            ChatCommand::Load { session_id, before } => {
                let session = self.database.session(&session_id)?;
                let end = before.unwrap_or(session.next_sequence);
                let after = (end > PAGE_SIZE as i64).then_some(end - PAGE_SIZE as i64 - 1);
                let mut page = self.database.messages(&session_id, PAGE_SIZE, after)?;
                page.messages.retain(|record| record.message.sequence < end);
                let context_tokens = self.database.context_tokens(&session.id)?;
                self.emit(ChatEvent::Loaded {
                    session,
                    page,
                    context_tokens,
                });
            }
            ChatCommand::ListModels => {
                let result = self.fetch_models();
                if !self.closed.load(Ordering::Acquire) {
                    self.emit(ChatEvent::Models(result));
                }
            }
            ChatCommand::SelectModel { model } => {
                ensure!(self.retained.is_none(), BusySnafu);
                ensure!(
                    !model.trim().is_empty(),
                    OptionsSnafu {
                        reason: "model must not be empty"
                    }
                );
                let model_port = self
                    .provider_ports
                    .as_ref()
                    .context(OptionsSnafu {
                        reason: "workflow does not support model selection",
                    })?
                    .model
                    .clone();
                self.parameters.0.insert(
                    model_port.clone(),
                    mf_runtime::ValueRef::from(model.as_str()),
                );
                let warning = preferences::persist_selected_model(
                    &preferences::preferences_path(self.database.path()),
                    &self.launch.options.profile,
                    &self.launch.contract.parameters,
                    &model_port,
                    &model,
                )
                .err()
                .map(|error| error.to_string());
                let cached_window = self.cached_context_window(&model);
                self.emit(ChatEvent::ModelSelected {
                    model: Some(model.clone()),
                    context_window: cached_window.flatten(),
                    automatic: false,
                    warning,
                });
                if cached_window.is_none()
                    && let Some(window) = self.probe_model_context_window(&model)
                    && !self.closed.load(Ordering::Acquire)
                {
                    self.emit(ChatEvent::ContextWindow {
                        model,
                        context_window: Some(window),
                    });
                }
            }
            ChatCommand::Submit { session_id, prompt } => {
                ensure!(
                    !prompt.trim().is_empty(),
                    OptionsSnafu {
                        reason: "prompt must not be empty"
                    }
                );
                ensure!(self.retained.is_none(), BusySnafu);
                let session = if let Some(id) = session_id {
                    self.database.session(&id)?
                } else {
                    let title: String = prompt
                        .trim()
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(48)
                        .collect();
                    self.database.create_provisional_session(
                        &self.launch.options.profile,
                        None,
                        title,
                    )?
                };
                self.submit(session, prompt, None)?;
            }
            ChatCommand::Retry { turn_id } => {
                ensure!(self.retained.is_none(), BusySnafu);
                let (turn, prompt) = self.database.retry_prompt(&turn_id)?;
                self.submit(
                    self.database.session(&turn.session_id)?,
                    prompt,
                    Some(turn_id),
                )?;
            }
            ChatCommand::Save => self.recover(false)?,
            ChatCommand::Abandon => self.recover(true)?,
        }
        Ok(())
    }
    fn submit(
        &mut self,
        session: Session,
        prompt: String,
        retry_of: Option<String>,
    ) -> Result<(), ChatRunError> {
        let parameters = mf_runtime::resolve_workflow_inputs(
            &self.launch.contract.parameters,
            &self.parameters,
            |name| std::env::var_os(name),
        )?;
        let invocation = Invocation::new(&session.id);
        if self.launch.contract.has_previews() {
            self.preview.lock().unwrap().begin(invocation.clone());
        }
        let captured = Arc::new(Mutex::new(None));
        let cancellation = mf_runtime::StreamCancellation::default();
        let backend = ExternalBackend {
            cancellation: cancellation.clone(),
            launch: Arc::clone(&self.launch),
            parameters,
            session: session.clone(),
            output: self.output.clone(),
            closed: Arc::clone(&self.closed),
            cancel: Arc::clone(&self.cancel),
            captured: Arc::clone(&captured),
        };
        let request = mf_chat::nodes::BeginTurnInputs {
            database_path: self
                .database
                .path()
                .to_str()
                .context(OptionsSnafu {
                    reason: "database path must be UTF-8",
                })?
                .into(),
            session_id: session.id.clone(),
            turn_id: invocation.turn_id.clone(),
            call_id: invocation.call_id.clone(),
            prompt,
            model: mf_chat::ModelIdentity {
                profile: session.profile,
                model: None,
            },
            retry_of,
            result_alternatives: Some(self.launch.contract.alternatives),
        };
        let preview_consumer = self.launch.contract.has_previews().then(|| {
            let outer_cancellation = cancellation.clone();
            let preview = self.preview.clone();
            let closed = self.closed.clone();
            let cancel = self.cancel.clone();
            mf_chat::PreviewConsumer::new(move |event, cancellation| {
                if let Some(error) = cancellation.failure() {
                    return Err(error.into());
                }
                if cancel.load(Ordering::Acquire) || closed.load(Ordering::Acquire) {
                    outer_cancellation.cancel(
                        mf_runtime::StreamExecutionSnafu {
                            message: "chat request cancelled",
                        }
                        .build(),
                    );
                    return Err(outer_cancellation
                        .failure()
                        .expect("cancellation was recorded")
                        .into());
                }
                // Consume and compact independently of terminal refresh or inspection.
                preview.lock().unwrap().apply(event)
            })
        });
        let resolution = mf_chat::workflow::run_conversation_with_preview(
            &self.database,
            request,
            Arc::new(backend),
            cancellation,
            true,
            None,
            preview_consumer,
        );
        let outcome = captured
            .lock()
            .expect("capture state was not poisoned")
            .take();
        if outcome.is_none() {
            let mut resolution = resolution?;
            ensure!(
                !self.cancel.load(Ordering::Acquire) && !self.closed.load(Ordering::Acquire),
                CancelledSnafu
            );
            return Err(resolution.diagnostics.remove(0).into());
        }
        let status = outcome.as_ref().map(|outcome| outcome.status);
        let stderr = outcome
            .as_ref()
            .map(|outcome| outcome.stderr.clone())
            .unwrap_or_default();
        let capture_error = outcome
            .as_ref()
            .and_then(|outcome| outcome.capture_error.clone());
        self.retained = match &resolution {
            Ok(ReconciledCompletion {
                state: CompletionState::Uncommitted(pending),
                ..
            }) => Some(Retained {
                invocation: invocation.clone(),
                response: RetainedResponse::Ready(pending.clone()),
            }),
            Err(_) => Some(Retained {
                invocation: invocation.clone(),
                response: RetainedResponse::Exited(
                    outcome.expect("failed reconciliation retains the exit"),
                ),
            }),
            _ => None,
        };
        self.busy.store(self.retained.is_some(), Ordering::Release);
        self.emit(ChatEvent::Finished {
            invocation,
            resolution,
            status,
            stderr,
            capture_error,
        });
        Ok(())
    }
    fn recover(&mut self, abandon: bool) -> Result<(), ChatRunError> {
        let retained = self.retained.as_ref().context(MissingReplySnafu)?;
        let invocation = retained.invocation.clone();
        let mut resolution = match &retained.response {
            RetainedResponse::Ready(pending) => ReconciledCompletion {
                state: CompletionState::Uncommitted(pending.clone()),
                diagnostics: vec![],
            },
            RetainedResponse::Exited(outcome) => {
                reconcile_turn(&self.database, &invocation, outcome)?
            }
        };
        if let CompletionState::Uncommitted(pending) = resolution.state {
            self.retained.as_mut().unwrap().response = RetainedResponse::Ready(pending.clone());
            resolution.state = if abandon {
                self.database.abandon_recovered(&pending)?;
                self.database
                    .reconcile_completion(ConversationExit {
                        invocation: &invocation,
                        stdout: None,
                        outcome: TurnExit::Cancelled,
                        summary: "reply discarded",
                    })?
                    .state
            } else {
                CompletionState::Committed(self.database.save_recovered(pending)?)
            };
        }
        self.retained = None;
        self.busy.store(false, Ordering::Release);
        self.emit(ChatEvent::Finished {
            invocation,
            resolution: Ok(resolution),
            status: None,
            stderr: String::new(),
            capture_error: None,
        });
        Ok(())
    }
}

struct ExternalBackend {
    cancellation: mf_runtime::StreamCancellation,
    launch: Arc<ChatLaunch>,
    parameters: mf_runtime::WorkflowArguments,
    session: Session,
    output: SyncSender<ChatEvent>,
    closed: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    captured: Arc<Mutex<Option<LocalOutcome>>>,
}

impl mf_chat::ConversationBackend for ExternalBackend {
    fn registered_workflow(
        &self,
    ) -> Option<(
        &mf_runtime::WorkflowManifest,
        &mf_runtime::WorkflowArguments,
    )> {
        Some((self.launch.runner.manifest(), &self.parameters))
    }

    fn builtin_title_parameters(&self) -> Option<mf_runtime::WorkflowArguments> {
        self.launch
            .contract
            .builtin_title_parameters(&self.parameters)
    }

    fn invoke(
        &self,
        invocation: &Invocation,
        request: mf_chat::ConversationInputs,
        context: &mut mf_runtime::ExecutionContext,
        preview: Option<&mut dyn FnMut(mf_chat::PreviewDelta) -> Result<(), mf_chat::ChatError>>,
        _: Option<&mf_telemetry::observation::Observer>,
    ) -> Result<mf_chat::ConversationResult, mf_chat::ChatError> {
        emit(
            &self.output,
            &self.closed,
            ChatEvent::Submitted {
                session: self.session.clone(),
                invocation: invocation.clone(),
                prompt: request.prompt()?.to_owned(),
            },
        );
        let execution = self.execute(request, context, preview);
        let failure = match execution.as_ref().err() {
            Some(ChatRunError::Runner {
                source: RunnerError::Cancelled { .. },
            }) => TurnExit::Cancelled,
            Some(ChatRunError::Runner {
                source:
                    RunnerError::Run {
                        source: RunError::Supervise { .. },
                    },
            }) => TurnExit::Interrupted,
            _ => TurnExit::Failed,
        };
        let (outcome, protocol_failure) = execution.boxed().context(mf_chat::BackendSnafu {
            outcome: failure,
            reply: None,
        })?;
        let reason = if protocol_failure.is_some()
            && !self.cancel.load(Ordering::Acquire)
            && !self.closed.load(Ordering::Acquire)
        {
            TurnExit::Failed
        } else {
            exit_reason(&outcome)
        };
        let result = if let Some(failure) = protocol_failure {
            Err(Box::new(failure) as Box<dyn std::error::Error + Send + Sync>)
        } else if let Some(failure) = &outcome.capture_failure {
            Err(Box::new(failure.clone()) as Box<dyn std::error::Error + Send + Sync>)
        } else if !outcome.status.success() {
            mf_chat::CaptureSnafu {
                message: "workflow process exited unsuccessfully",
            }
            .fail()
            .boxed()
        } else {
            mf_chat::ConversationResult::from_json(&outcome.stdout).boxed()
        };
        let result = result.map(|mut response| {
            outcome.call_events.apply(&mut response.usages);
            response
        });
        let reply = if result.is_err() {
            mf_chat::ConversationResult::from_json(&outcome.stdout)
                .ok()
                .map(|mut response| {
                    outcome.call_events.apply(&mut response.usages);
                    Box::new(response)
                })
        } else {
            None
        };
        *self
            .captured
            .lock()
            .expect("capture state was not poisoned") = Some(outcome);
        result.context(mf_chat::BackendSnafu {
            outcome: reason,
            reply,
        })
    }
}

impl ExternalBackend {
    fn execute(
        &self,
        request: mf_chat::ConversationInputs,
        context: &mut mf_runtime::ExecutionContext,
        mut preview: Option<
            &mut dyn FnMut(mf_chat::PreviewDelta) -> Result<(), mf_chat::ChatError>,
        >,
    ) -> Result<(LocalOutcome, Option<mf_chat::ChatError>), ChatRunError> {
        let arguments = self.launch.contract.arguments(request, &self.parameters)?;
        let mut decoder = mf_chat::contract::ConversationOutputDecoder::new(&self.launch.contract);
        let completion = RunnerInvocation {
            runner: &self.launch.runner,
            arguments: &arguments,
            verbose: self.launch.options.run.verbose,
            snapshots: true,
        }
        .run(
            context.cancellation(),
            |observation| {
                emit(
                    &self.output,
                    &self.closed,
                    ChatEvent::Observing(observation),
                )
            },
            |record| {
                if let Some(delta) = decoder.accept(record)?
                    && let Some(consume) = preview.as_mut()
                {
                    consume(delta)?;
                }
                Ok(())
            },
            || {
                let cancelled =
                    self.cancel.load(Ordering::Acquire) || self.closed.load(Ordering::Acquire);
                if cancelled {
                    self.cancellation.cancel(
                        mf_runtime::StreamExecutionSnafu {
                            message: "chat request cancelled",
                        }
                        .build(),
                    );
                }
                cancelled
            },
        )?;
        let mut outcome = completion.outcome;
        let mut protocol_failure = completion.protocol_failure;
        if self.launch.contract.streaming {
            outcome.stdout = decoder
                .result()
                .map(serde_json::to_vec)
                .transpose()?
                .unwrap_or_default();
            if protocol_failure.is_none()
                && outcome.capture_error.is_none()
                && let Err(error) = decoder.finish()
            {
                protocol_failure = Some(error);
            }
        }
        Ok((outcome, protocol_failure))
    }
}

fn exit_reason(outcome: &LocalOutcome) -> TurnExit {
    if outcome.interrupted {
        TurnExit::Cancelled
    } else if outcome.status.code().is_none() {
        TurnExit::Interrupted
    } else {
        TurnExit::Failed
    }
}

fn reconcile_turn(
    database: &ChatDatabase,
    invocation: &Invocation,
    outcome: &LocalOutcome,
) -> Result<ReconciledCompletion, mf_chat::ChatError> {
    let reason = exit_reason(outcome);
    let stdout = outcome
        .capture_error
        .is_none()
        .then_some(outcome.stdout.as_slice())
        .or_else(|| {
            mf_chat::ConversationResult::from_json(&outcome.stdout)
                .is_ok()
                .then_some(outcome.stdout.as_slice())
        });
    let result = stdout.map(|bytes| {
        mf_chat::ConversationResult::from_json(bytes).map(|mut response| {
            outcome.call_events.apply(&mut response.usages);
            response
        })
    });
    database.reconcile_result(
        invocation,
        result,
        reason,
        if outcome.stderr.is_empty() {
            "runner exited without completion"
        } else {
            &outcome.stderr
        },
    )
}

#[cfg(test)]
mod tests;
