//! Persisted non-secret startup preferences and native provider model selection for chat hosts.

use crate::{
    ChatError,
    contract::{ChatContract, ProviderParameterPorts},
    error::*,
};
use mf_runtime::{
    ExecutionContext, FlowNode, Inputs, NodeMetadata, NodePorts, NodeRegistry, NodeResult,
    NodeValue, PortSpec, PreparedNode, StreamCancellation, StreamExecutionSnafu, ValueRef,
    ValueType, WorkflowArguments, WorkflowInputError, WorkflowInputSchema,
};
use mfn_llm::{LIST_MODELS_KIND, MODEL_INFO_KIND, ModelInfo, ModelInfoOutputs, ModelListOutputs};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use snafu::{OptionExt, ResultExt, ensure};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

const PREFERENCES_VERSION: u32 = 1;
const MAX_PREFERENCES_BYTES: usize = 1024 * 1024;
const MODEL_LIST_TIMEOUT_MS: u64 = 15_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatPreferencesStore {
    version: u32,
    #[serde(default)]
    profiles: BTreeMap<String, ProfilePreferences>,
}

impl Default for ChatPreferencesStore {
    fn default() -> Self {
        Self {
            version: PREFERENCES_VERSION,
            profiles: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfilePreferences {
    #[serde(default)]
    parameters: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
struct MergedParameters {
    effective: WorkflowArguments,
    updated_profile: ProfilePreferences,
    preferences_changed: bool,
    needs_default_model: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ModelListEndpoint {
    base_url: String,
    api_key: Option<String>,
}

/// Cross-thread handle that cancels in-flight provider model queries.
#[derive(Clone, Debug, Default)]
pub struct ModelSelectionCancellation {
    state: Arc<CancellationState>,
}

#[derive(Debug, Default)]
struct CancellationState {
    cancelled: AtomicBool,
    active: Mutex<Option<StreamCancellation>>,
}

impl ModelSelectionCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels only the currently in-flight query without preventing future queries.
    pub fn cancel_active(&self) {
        if let Some(active) = self.state.active.lock().unwrap().as_ref() {
            active.cancel(
                StreamExecutionSnafu {
                    message: "model query cancelled",
                }
                .build(),
            );
        }
    }

    /// Marks the handle permanently cancelled and aborts any in-flight query.
    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::Release);
        self.cancel_active();
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
    }

    fn begin_query(&self) -> StreamCancellation {
        let cancellation = StreamCancellation::default();
        *self.state.active.lock().unwrap() = Some(cancellation.clone());
        if self.is_cancelled() {
            cancellation.cancel(
                StreamExecutionSnafu {
                    message: "model query cancelled",
                }
                .build(),
            );
        }
        cancellation
    }

    fn finish_query(&self) {
        *self.state.active.lock().unwrap() = None;
    }
}

/// Result of selecting or automatically discovering the active chat model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSelectionUpdate {
    pub model: Option<String>,
    pub context_window: Option<u64>,
    pub warning: Option<String>,
}

/// Manages chat launch parameter preferences and interactive provider model selection.
pub struct ModelSelection {
    preferences_path: PathBuf,
    profile: String,
    schema: WorkflowInputSchema,
    provider_ports: Option<ProviderParameterPorts>,
    effective: WorkflowArguments,
    pending_profile: Option<ProfilePreferences>,
    needs_default_model: bool,
    list_models_node: Option<PreparedNode>,
    model_info_node: Option<PreparedNode>,
    context_windows: crate::ModelContextWindowCache,
    cancellation: ModelSelectionCancellation,
}

impl ModelSelection {
    /// Loads saved preferences, merges and validates explicit launch parameters, and prepares
    /// the registered `llm.list_models` node when the contract exposes provider ports.
    ///
    /// Does not write preferences to disk; call [`Self::commit_startup_preferences`] once the
    /// caller holds database ownership.
    pub fn prepare(
        database_path: &Path,
        profile: impl Into<String>,
        contract: &ChatContract,
        explicit: &WorkflowArguments,
    ) -> Result<Self, ChatError> {
        let profile = profile.into();
        let prefs_path = preferences_path(database_path);
        let provider_ports = contract.provider_parameter_ports();
        let merged = resolve_launch_preferences(
            &contract.parameters,
            provider_ports.as_ref(),
            &prefs_path,
            &profile,
            explicit,
        )?;
        let list_models_node = provider_ports
            .is_some()
            .then(prepare_list_models_node)
            .transpose()?;
        let model_info_node = provider_ports
            .is_some()
            .then(prepare_model_info_node)
            .transpose()?;
        Ok(Self {
            preferences_path: prefs_path,
            profile,
            schema: contract.parameters.clone(),
            provider_ports,
            effective: merged.effective,
            pending_profile: merged.preferences_changed.then_some(merged.updated_profile),
            needs_default_model: merged.needs_default_model,
            list_models_node,
            model_info_node,
            context_windows: crate::ModelContextWindowCache::default(),
            cancellation: ModelSelectionCancellation::new(),
        })
    }

    /// Persists startup parameter changes computed during [`Self::prepare`], if any.
    pub fn commit_startup_preferences(&mut self) -> Result<(), ChatError> {
        let Some(profile_prefs) = self.pending_profile.take() else {
            return Ok(());
        };
        if let Err(error) =
            persist_profile(&self.preferences_path, &self.profile, profile_prefs.clone())
        {
            self.pending_profile = Some(profile_prefs);
            return Err(error);
        }
        Ok(())
    }

    pub fn cancellation(&self) -> ModelSelectionCancellation {
        self.cancellation.clone()
    }

    pub fn supports_model_selection(&self) -> bool {
        self.provider_ports.is_some()
    }

    pub fn needs_default_model(&self) -> bool {
        self.needs_default_model
    }

    pub fn display_model(&self) -> Option<String> {
        self.provider_ports.as_ref().and_then(|ports| {
            resolve_display_model(&self.schema, ports, &self.effective, |name| {
                std::env::var_os(name)
            })
        })
    }

    pub fn effective_parameters(&self) -> &WorkflowArguments {
        &self.effective
    }

    /// Lists available models from the configured provider endpoint using the registered
    /// `llm.list_models` node.
    pub fn list_models(&mut self) -> Result<Vec<ModelInfo>, ChatError> {
        let node = self.list_models_node.as_ref().context(InvalidSnafu {
            field: "model selection",
            reason: "workflow does not expose model selection parameters",
        })?;
        let endpoint = self.resolved_endpoint().context(InvalidSnafu {
            field: "model selection",
            reason: "provider base_url is not configured",
        })?;
        let base_url = endpoint.base_url.clone();
        let cancellation = self.cancellation.begin_query();
        let executed = model_list_context(endpoint, cancellation).and_then(|mut context| {
            let task = node
                .execution
                .as_task_node()
                .expect("llm.list_models is a task node");
            task.execute(Inputs::new(), &mut context)
        });
        self.cancellation.finish_query();
        let executed = executed?;
        let outputs = <ModelListOutputs as NodeValue>::from_values(executed.outputs)?;
        self.context_windows
            .record_models(&base_url, &outputs.models);
        Ok(outputs.models)
    }

    /// Returns a handle to the shared per-endpoint model context window cache.
    pub fn context_windows(&self) -> crate::ModelContextWindowCache {
        self.context_windows.clone()
    }

    /// Returns the cached context window lookup state for `model` on the current endpoint.
    ///
    /// Returns `None` if no lookup has been cached yet, `Some(None)` if the endpoint did not
    /// advertise a positive window, and `Some(Some(tokens))` if a positive window is known.
    pub fn cached_context_window(&self, model: &str) -> Option<Option<u64>> {
        let endpoint = self.resolved_endpoint()?;
        self.context_windows.get(&endpoint.base_url, model)
    }

    /// Resolves the context window for `model`, reusing the per-endpoint cache or querying the
    /// registered `llm.model_info` node.
    pub fn probe_context_window(&mut self, model: &str) -> Option<u64> {
        if let Some(cached) = self.cached_context_window(model) {
            return cached;
        }
        let endpoint = self.resolved_endpoint()?;
        let base_url = endpoint.base_url.clone();
        let node = self.model_info_node.as_ref()?;
        let cancellation = self.cancellation.begin_query();
        let executed =
            model_info_context(model, endpoint, cancellation.clone()).and_then(|mut context| {
                let task = node
                    .execution
                    .as_task_node()
                    .expect("llm.model_info is a task node");
                task.execute(Inputs::new(), &mut context)
            });
        self.cancellation.finish_query();
        if cancellation.failure().is_some() || self.cancellation.is_cancelled() {
            return None;
        }
        let discovered = executed
            .ok()
            .and_then(|result| <ModelInfoOutputs as NodeValue>::from_values(result.outputs).ok())
            .and_then(|outputs| positive_context_window(outputs.context_window));
        self.context_windows.record(&base_url, model, discovered);
        discovered
    }

    /// Selects the first available provider model when startup has no explicit/saved/env model,
    /// falling back to the schema default if discovery fails or returns no models.
    pub fn discover_default_model(&mut self) -> ModelSelectionUpdate {
        self.needs_default_model = false;
        let fetched = self.list_models();
        if self.cancellation.is_cancelled() {
            return ModelSelectionUpdate {
                model: None,
                context_window: None,
                warning: None,
            };
        }
        if let Ok(models) = fetched
            && let Some(first) = models.into_iter().find(|info| !info.id.trim().is_empty())
            && let Some(ports) = &self.provider_ports
        {
            self.effective
                .0
                .insert(ports.model.clone(), ValueRef::from(first.id.as_str()));
            let warning = persist_selected_model(
                &self.preferences_path,
                &self.profile,
                &self.schema,
                &ports.model,
                &first.id,
            )
            .err()
            .map(|error| error.to_string());
            return ModelSelectionUpdate {
                model: Some(first.id),
                context_window: positive_context_window(first.context_window),
                warning,
            };
        }
        let fallback = self.display_model();
        let context_window = fallback
            .as_deref()
            .and_then(|model| self.cached_context_window(model).flatten());
        ModelSelectionUpdate {
            model: fallback,
            context_window,
            warning: None,
        }
    }

    /// Updates the active model parameter in memory and attempts to persist it for future runs.
    pub fn select_model(&mut self, model: &str) -> Result<ModelSelectionUpdate, ChatError> {
        ensure!(
            !model.trim().is_empty(),
            InvalidSnafu {
                field: "model",
                reason: "model must not be empty",
            }
        );
        let model_port = self
            .provider_ports
            .as_ref()
            .context(InvalidSnafu {
                field: "model selection",
                reason: "workflow does not support model selection",
            })?
            .model
            .clone();
        self.effective
            .0
            .insert(model_port.clone(), ValueRef::from(model));
        self.needs_default_model = false;
        let warning = persist_selected_model(
            &self.preferences_path,
            &self.profile,
            &self.schema,
            &model_port,
            model,
        )
        .err()
        .map(|error| error.to_string());
        let context_window = self.cached_context_window(model).flatten();
        Ok(ModelSelectionUpdate {
            model: Some(model.to_owned()),
            context_window,
            warning,
        })
    }

    fn resolved_endpoint(&self) -> Option<ModelListEndpoint> {
        let ports = self.provider_ports.as_ref()?;
        resolve_model_list_endpoint(&self.schema, ports, &self.effective, |name| {
            std::env::var_os(name)
        })
    }
}

fn prepare_list_models_node() -> Result<PreparedNode, ChatError> {
    let registry = NodeRegistry::from_inventory()?;
    let factory = registry.get(LIST_MODELS_KIND).context(InvalidSnafu {
        field: "model selection",
        reason: "llm.list_models node is not registered",
    })?;
    factory
        .instantiate(serde_json::json!({
            "base_url": {"$ref": "params.base_url"},
            "api_key": {"$ref": "params.api_key"},
            "timeout_ms": MODEL_LIST_TIMEOUT_MS,
        }))
        .context(BackendBuildSnafu)
}

fn prepare_model_info_node() -> Result<PreparedNode, ChatError> {
    let registry = NodeRegistry::from_inventory()?;
    let factory = registry.get(MODEL_INFO_KIND).context(InvalidSnafu {
        field: "model selection",
        reason: "llm.model_info node is not registered",
    })?;
    factory
        .instantiate(serde_json::json!({
            "model": {"$ref": "params.model"},
            "base_url": {"$ref": "params.base_url"},
            "api_key": {"$ref": "params.api_key"},
            "timeout_ms": MODEL_LIST_TIMEOUT_MS,
        }))
        .context(BackendBuildSnafu)
}

fn positive_context_window(window: Option<i64>) -> Option<u64> {
    window
        .and_then(|value| u64::try_from(value).ok())
        .filter(|&value| value > 0)
}

fn model_info_context(
    model: &str,
    endpoint: ModelListEndpoint,
    cancellation: StreamCancellation,
) -> Result<ExecutionContext, mf_runtime::NodeExecutionError> {
    let source: FlowNode<()> = FlowNode {
        definition_id: "params".into(),
        node: (),
        metadata: NodeMetadata {
            ports: NodePorts {
                inputs: Vec::new(),
                outputs: vec![
                    PortSpec::new("model", ValueType::String, true),
                    PortSpec::new("base_url", ValueType::String, true),
                    PortSpec::new("api_key", ValueType::String, true),
                ],
            },
            ..Default::default()
        },
        output_keys: OnceLock::new(),
    };
    let mut context = ExecutionContext::for_message(
        &source,
        NodeResult {
            outputs: [
                ("model".into(), ValueRef::from(model)),
                ("base_url".into(), ValueRef::from(endpoint.base_url)),
                (
                    "api_key".into(),
                    ValueRef::from(endpoint.api_key.unwrap_or_default()).into_secret(),
                ),
            ]
            .into(),
            ..Default::default()
        },
    )?;
    context.set_cancellation(cancellation);
    Ok(context)
}

fn model_list_context(
    endpoint: ModelListEndpoint,
    cancellation: StreamCancellation,
) -> Result<ExecutionContext, mf_runtime::NodeExecutionError> {
    let source: FlowNode<()> = FlowNode {
        definition_id: "params".into(),
        node: (),
        metadata: NodeMetadata {
            ports: NodePorts {
                inputs: Vec::new(),
                outputs: vec![
                    PortSpec::new("base_url", ValueType::String, true),
                    PortSpec::new("api_key", ValueType::String, true),
                ],
            },
            ..Default::default()
        },
        output_keys: OnceLock::new(),
    };
    let mut context = ExecutionContext::for_message(
        &source,
        NodeResult {
            outputs: [
                ("base_url".into(), ValueRef::from(endpoint.base_url)),
                (
                    "api_key".into(),
                    ValueRef::from(endpoint.api_key.unwrap_or_default()).into_secret(),
                ),
            ]
            .into(),
            ..Default::default()
        },
    )?;
    context.set_cancellation(cancellation);
    Ok(context)
}

fn preferences_path(database_path: &Path) -> PathBuf {
    let mut name = database_path.as_os_str().to_os_string();
    name.push(".preferences.json");
    PathBuf::from(name)
}

fn load_store(path: &Path) -> Result<ChatPreferencesStore, ChatError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ChatPreferencesStore::default());
        }
        Err(error) => {
            return Err(error).context(IoSnafu {
                operation: "open chat preferences",
                path: path.to_path_buf(),
            });
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_PREFERENCES_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .context(IoSnafu {
            operation: "read chat preferences",
            path: path.to_path_buf(),
        })?;
    ensure!(
        bytes.len() <= MAX_PREFERENCES_BYTES,
        PreferencesSnafu {
            path: path.to_path_buf(),
            reason: "preferences file exceeds the 1 MiB size limit",
        }
    );
    let store: ChatPreferencesStore =
        mf_runtime::decode_unique_json(&bytes).context(PreferencesJsonSnafu {
            path: path.to_path_buf(),
        })?;
    ensure!(
        store.version == PREFERENCES_VERSION,
        PreferencesSnafu {
            path: path.to_path_buf(),
            reason: format!("unsupported preferences version {}", store.version),
        }
    );
    Ok(store)
}

fn save_store(path: &Path, store: &ChatPreferencesStore) -> Result<(), ChatError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut bytes = serde_json::to_vec_pretty(store).context(JsonSnafu)?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= MAX_PREFERENCES_BYTES,
        PreferencesSnafu {
            path: path.to_path_buf(),
            reason: "serialized preferences exceed the 1 MiB size limit",
        }
    );
    let mut temp = tempfile::Builder::new()
        .prefix(".chat-preferences-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .context(IoSnafu {
            operation: "create temporary chat preferences file",
            path: parent.to_path_buf(),
        })?;
    temp.write_all(&bytes).context(IoSnafu {
        operation: "write chat preferences",
        path: path.to_path_buf(),
    })?;
    temp.as_file().sync_all().context(IoSnafu {
        operation: "sync chat preferences",
        path: path.to_path_buf(),
    })?;
    temp.persist(path)
        .map_err(|error| error.error)
        .context(IoSnafu {
            operation: "replace chat preferences",
            path: path.to_path_buf(),
        })?;
    Ok(())
}

fn is_persistable_parameter(schema: &WorkflowInputSchema, name: &str, value: &ValueRef) -> bool {
    let Some(input) = schema.parameter(name) else {
        return false;
    };
    !input.binding.secret && !input.value_type.is_runtime_only() && !value.contains_secret()
}

fn merge_parameters(
    schema: &WorkflowInputSchema,
    provider_ports: Option<&ProviderParameterPorts>,
    saved: &ProfilePreferences,
    explicit: &WorkflowArguments,
    mut environment: impl FnMut(&str) -> Option<OsString>,
) -> Result<MergedParameters, ChatError> {
    let base_url_changed = provider_ports.is_some_and(|ports| {
        if explicit.0.contains_key(&ports.model) {
            return false;
        }
        let Some(explicit_base) = explicit.0.get(&ports.base_url).and_then(ValueRef::as_str) else {
            return false;
        };
        let previous_base = saved
            .parameters
            .get(&ports.base_url)
            .and_then(Value::as_str)
            .or_else(|| {
                schema
                    .parameter(&ports.base_url)
                    .and_then(|input| input.binding.default.as_ref())
                    .and_then(Value::as_str)
            });
        previous_base.map(|value| value.trim_end_matches('/'))
            != Some(explicit_base.trim_end_matches('/'))
    });

    let mut effective = WorkflowArguments::default();
    for (name, value) in &saved.parameters {
        if base_url_changed && provider_ports.is_some_and(|ports| ports.model == *name) {
            continue;
        }
        let Some(input) = schema.parameter(name) else {
            continue;
        };
        if input.binding.secret || input.value_type.is_runtime_only() {
            continue;
        }
        let value_ref: ValueRef = value.clone().into();
        if input.value_type.validate_shared(&value_ref).is_ok() {
            effective.0.insert(name.clone(), value_ref);
        }
    }

    let mut updated_profile = saved.clone();
    if base_url_changed && let Some(ports) = provider_ports {
        updated_profile.parameters.remove(&ports.model);
    }

    for (name, value) in &explicit.0 {
        effective.0.insert(name.clone(), value.clone());
        if is_persistable_parameter(schema, name, value) {
            updated_profile.parameters.insert(
                name.clone(),
                serde_json::to_value(value).context(JsonSnafu)?,
            );
        }
    }
    schema.mark_secrets(&mut effective);

    let needs_default_model = provider_ports.is_some_and(|ports| {
        if effective.0.contains_key(&ports.model) {
            return false;
        }
        let env_configured = schema
            .parameter(&ports.model)
            .and_then(|input| input.binding.env.as_deref())
            .and_then(&mut environment)
            .and_then(|value| value.into_string().ok())
            .is_some_and(|value| !value.trim().is_empty());
        !env_configured
    });

    let preferences_changed = updated_profile != *saved;
    Ok(MergedParameters {
        effective,
        updated_profile,
        preferences_changed,
        needs_default_model,
    })
}

fn validate_launch_parameters(
    schema: &WorkflowInputSchema,
    provider_ports: Option<&ProviderParameterPorts>,
    merged: &WorkflowArguments,
) -> Result<(), WorkflowInputError> {
    schema.validate_values(merged)?;
    if let Some(ports) = provider_ports
        && !merged.0.contains_key(&ports.model)
    {
        // Defer required-model validation when startup can discover the first provider model.
        let mut probe = merged.clone();
        probe
            .0
            .insert(ports.model.clone(), ValueRef::from("placeholder"));
        schema.validate(&probe)
    } else {
        schema.validate(merged)
    }
}

fn resolve_launch_preferences(
    schema: &WorkflowInputSchema,
    provider_ports: Option<&ProviderParameterPorts>,
    prefs_path: &Path,
    profile: &str,
    explicit: &WorkflowArguments,
) -> Result<MergedParameters, ChatError> {
    let saved_store = load_store(prefs_path)?;
    let saved = saved_store
        .profiles
        .get(profile)
        .cloned()
        .unwrap_or_default();
    let merged = merge_parameters(schema, provider_ports, &saved, explicit, |name| {
        std::env::var_os(name)
    })?;
    validate_launch_parameters(schema, provider_ports, &merged.effective)?;
    Ok(merged)
}

fn resolve_string_parameter(
    schema: &WorkflowInputSchema,
    parameters: &WorkflowArguments,
    name: &str,
    mut environment: impl FnMut(&str) -> Option<OsString>,
) -> Option<String> {
    if let Some(value) = parameters.0.get(name).and_then(ValueRef::as_str) {
        return Some(value.to_owned());
    }
    let input = schema.parameter(name)?;
    if let Some(env_name) = input.binding.env.as_deref()
        && let Some(value) = environment(env_name)
        && let Ok(text) = value.into_string()
    {
        return Some(text);
    }
    input
        .binding
        .default
        .as_ref()
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn resolve_model_list_endpoint(
    schema: &WorkflowInputSchema,
    provider_ports: &ProviderParameterPorts,
    parameters: &WorkflowArguments,
    mut environment: impl FnMut(&str) -> Option<OsString>,
) -> Option<ModelListEndpoint> {
    let base_url = resolve_string_parameter(
        schema,
        parameters,
        &provider_ports.base_url,
        &mut environment,
    )?;
    if base_url.trim().is_empty() {
        return None;
    }
    let api_key = provider_ports
        .api_key
        .as_deref()
        .and_then(|port| resolve_string_parameter(schema, parameters, port, &mut environment))
        .filter(|value| !value.is_empty());
    Some(ModelListEndpoint { base_url, api_key })
}

fn resolve_display_model(
    schema: &WorkflowInputSchema,
    provider_ports: &ProviderParameterPorts,
    parameters: &WorkflowArguments,
    environment: impl FnMut(&str) -> Option<OsString>,
) -> Option<String> {
    resolve_string_parameter(schema, parameters, &provider_ports.model, environment)
        .filter(|value| !value.trim().is_empty())
}

fn persist_profile(
    path: &Path,
    profile: &str,
    preferences: ProfilePreferences,
) -> Result<(), ChatError> {
    let mut store = load_store(path)?;
    store.profiles.insert(profile.to_owned(), preferences);
    save_store(path, &store)
}

fn persist_selected_model(
    path: &Path,
    profile: &str,
    schema: &WorkflowInputSchema,
    model_port: &str,
    model: &str,
) -> Result<(), ChatError> {
    let value = ValueRef::from(model);
    if !is_persistable_parameter(schema, model_port, &value) {
        return Ok(());
    }
    let mut store = load_store(path)?;
    let entry = store.profiles.entry(profile.to_owned()).or_default();
    if entry.parameters.get(model_port).and_then(Value::as_str) == Some(model) {
        return Ok(());
    }
    entry
        .parameters
        .insert(model_port.to_owned(), Value::String(model.to_owned()));
    save_store(path, &store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mfn_llm::test_support::{Reply, Server};
    use serde_json::json;

    fn sample_schema() -> WorkflowInputSchema {
        serde_json::from_value(json!({
            "inputs": {
                "params": {
                    "model": {
                        "type": "string",
                        "required": true,
                        "binding": {"default": "your-model", "secret": false}
                    },
                    "base_url": {
                        "type": "string",
                        "required": true,
                        "binding": {"default": "https://api.openai.com/v1", "secret": false}
                    },
                    "api_key": {
                        "type": "string",
                        "required": false,
                        "binding": {"env": "OPENAI_API_KEY", "default": "", "secret": true}
                    },
                    "temperature": {
                        "type": "number",
                        "required": false,
                        "binding": {"secret": false}
                    }
                }
            },
            "stdin": {}
        }))
        .unwrap()
    }

    fn sample_ports() -> ProviderParameterPorts {
        ProviderParameterPorts {
            model: "params.model".into(),
            base_url: "params.base_url".into(),
            api_key: Some("params.api_key".into()),
        }
    }

    fn model_selectable_contract() -> ChatContract {
        let definition: mf_runtime::WorkflowDefinition = serde_json::from_value(json!({
            "version": "2026-10-03",
            "dependencies": {},
            "nodes": [
                {"id": "params", "kind": "builtin.identity", "config": {}},
                {"id": "start", "kind": "builtin.llm", "config": {"model": "fixture-model"}}
            ],
            "outputs": [
                {"name": "append", "node": "start", "port": "append"}
            ]
        }))
        .unwrap();
        let description = mf_runtime::describe_graph(&mf_runtime::CompiledGraph {
            definition: definition.clone(),
            execution_order: vec!["params".into(), "start".into()],
            relations: Default::default(),
            children: Default::default(),
        })
        .unwrap();
        let input_schema: WorkflowInputSchema = serde_json::from_value(json!({
            "inputs": {
                "start": {
                    "messages": {
                        "type": ValueType::List(Box::new(ValueType::Object)),
                        "required": true
                    }
                },
                "params": {
                    "model": {
                        "type": "string",
                        "required": true,
                        "binding": {
                            "default": "fallback-model",
                            "env": "MF_TEST_UNSET_CHAT_MODEL",
                            "secret": false
                        }
                    },
                    "base_url": {
                        "type": "string",
                        "required": true,
                        "binding": {
                            "default": "",
                            "env": "MF_TEST_UNSET_CHAT_BASE_URL",
                            "secret": false
                        }
                    },
                    "api_key": {
                        "type": "string",
                        "required": false,
                        "binding": {
                            "default": "",
                            "env": "MF_TEST_UNSET_CHAT_API_KEY",
                            "secret": true
                        }
                    }
                }
            },
            "stdin": {}
        }))
        .unwrap();
        let manifest = mf_runtime::WorkflowManifest {
            version: mf_runtime::WorkflowManifestVersion::V2026_11_25,
            definition,
            entry: "chat.json".into(),
            sources: BTreeMap::new(),
            descriptor: mf_runtime::RunnerDescriptor {
                description,
                input_schema,
                node_targets: Vec::new(),
            },
        };
        ChatContract::from_manifest(&manifest).unwrap()
    }

    #[test]
    fn merge_persists_non_secret_explicit_inputs_and_never_persists_secrets() {
        let schema = sample_schema();
        let ports = sample_ports();
        let explicit = WorkflowArguments(BTreeMap::from([
            (
                "params.base_url".into(),
                ValueRef::from("http://127.0.0.1:8080/v1"),
            ),
            ("params.api_key".into(), ValueRef::from("secret-token")),
            ("params.temperature".into(), ValueRef::from(json!(0.25))),
        ]));
        let merged = merge_parameters(
            &schema,
            Some(&ports),
            &ProfilePreferences::default(),
            &explicit,
            |_| None,
        )
        .unwrap();
        assert!(merged.preferences_changed);
        assert!(merged.needs_default_model);
        assert_eq!(
            merged
                .updated_profile
                .parameters
                .get("params.base_url")
                .and_then(Value::as_str),
            Some("http://127.0.0.1:8080/v1")
        );
        assert_eq!(
            merged.updated_profile.parameters.get("params.temperature"),
            Some(&json!(0.25))
        );
        assert!(
            !merged
                .updated_profile
                .parameters
                .contains_key("params.api_key")
        );
        assert!(merged.effective.0["params.api_key"].is_secret());
    }

    #[test]
    fn merge_reuses_saved_startup_config_and_invalidates_stale_model_on_endpoint_change() {
        let schema = sample_schema();
        let ports = sample_ports();
        let saved = ProfilePreferences {
            parameters: BTreeMap::from([
                ("params.base_url".into(), json!("http://127.0.0.1:8080/v1")),
                ("params.model".into(), json!("saved-model")),
                ("params.unknown".into(), json!("from-other-flow")),
            ]),
        };

        let reused = merge_parameters(
            &schema,
            Some(&ports),
            &saved,
            &WorkflowArguments::default(),
            |_| None,
        )
        .unwrap();
        assert!(!reused.preferences_changed);
        assert!(!reused.needs_default_model);
        assert_eq!(
            reused
                .effective
                .0
                .get("params.base_url")
                .and_then(ValueRef::as_str),
            Some("http://127.0.0.1:8080/v1")
        );
        assert_eq!(
            reused
                .effective
                .0
                .get("params.model")
                .and_then(ValueRef::as_str),
            Some("saved-model")
        );
        assert!(!reused.effective.0.contains_key("params.unknown"));
        assert!(
            reused
                .updated_profile
                .parameters
                .contains_key("params.unknown")
        );

        let switched_endpoint = WorkflowArguments(BTreeMap::from([(
            "params.base_url".into(),
            ValueRef::from("http://127.0.0.1:9090/v1"),
        )]));
        let changed =
            merge_parameters(&schema, Some(&ports), &saved, &switched_endpoint, |_| None).unwrap();
        assert!(changed.preferences_changed);
        assert!(changed.needs_default_model);
        assert!(!changed.effective.0.contains_key("params.model"));
        assert!(
            !changed
                .updated_profile
                .parameters
                .contains_key("params.model")
        );
        assert_eq!(
            changed
                .updated_profile
                .parameters
                .get("params.base_url")
                .and_then(Value::as_str),
            Some("http://127.0.0.1:9090/v1")
        );
    }

    #[test]
    fn save_and_load_round_trip_and_reject_corrupt_or_oversized_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = preferences_path(&dir.path().join("chat.sqlite"));
        assert_eq!(load_store(&path).unwrap(), ChatPreferencesStore::default());

        let schema = sample_schema();
        persist_selected_model(&path, "default", &schema, "params.model", "model-a").unwrap();
        persist_selected_model(&path, "work", &schema, "params.model", "model-b").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let loaded = load_store(&path).unwrap();
        assert_eq!(
            loaded.profiles["default"].parameters["params.model"],
            json!("model-a")
        );
        assert_eq!(
            loaded.profiles["work"].parameters["params.model"],
            json!("model-b")
        );

        std::fs::write(&path, "{invalid json").unwrap();
        assert!(matches!(
            load_store(&path),
            Err(ChatError::PreferencesJson { .. })
        ));

        std::fs::write(&path, json!({"version": 99, "profiles": {}}).to_string()).unwrap();
        assert!(matches!(
            load_store(&path),
            Err(ChatError::Preferences { .. })
        ));

        std::fs::write(&path, vec![b' '; MAX_PREFERENCES_BYTES + 1]).unwrap();
        assert!(matches!(
            load_store(&path),
            Err(ChatError::Preferences { .. })
        ));
        let dir_path = dir.path().join("not-a-file");
        std::fs::create_dir(&dir_path).unwrap();
        assert!(matches!(load_store(&dir_path), Err(ChatError::Io { .. })));

        // Opening a path whose parent is a regular file triggers a non-NotFound open error.
        let not_a_dir = path.join("child");
        assert!(matches!(load_store(&not_a_dir), Err(ChatError::Io { .. })));

        let mut oversized = ChatPreferencesStore::default();
        oversized.profiles.insert(
            "default".into(),
            ProfilePreferences {
                parameters: BTreeMap::from([(
                    "params.model".into(),
                    Value::String("x".repeat(MAX_PREFERENCES_BYTES + 1)),
                )]),
            },
        );
        assert!(matches!(
            save_store(&path, &oversized),
            Err(ChatError::Preferences { .. })
        ));

        let clean_path = preferences_path(&dir.path().join("clean.sqlite"));
        persist_profile(
            &clean_path,
            "default",
            ProfilePreferences {
                parameters: BTreeMap::from([("params.model".into(), json!("model-a"))]),
            },
        )
        .unwrap();
        persist_selected_model(&clean_path, "default", &schema, "params.model", "model-a").unwrap();
        persist_selected_model(&clean_path, "default", &schema, "params.api_key", "secret")
            .unwrap();
        persist_selected_model(&clean_path, "default", &schema, "params.unknown", "x").unwrap();
        let clean_loaded = load_store(&clean_path).unwrap();
        assert_eq!(clean_loaded.profiles["default"].parameters.len(), 1);
    }

    #[test]
    fn resolve_helpers_apply_explicit_env_and_default_precedence() {
        let schema = sample_schema();
        let ports = sample_ports();
        let empty = WorkflowArguments::default();

        assert_eq!(
            resolve_display_model(&schema, &ports, &empty, |_| None).as_deref(),
            Some("your-model")
        );
        let default_endpoint =
            resolve_model_list_endpoint(&schema, &ports, &empty, |_| None).unwrap();
        assert_eq!(default_endpoint.base_url, "https://api.openai.com/v1");
        assert_eq!(default_endpoint.api_key, None);

        let env_endpoint = resolve_model_list_endpoint(&schema, &ports, &empty, |name| {
            (name == "OPENAI_API_KEY").then(|| OsString::from("env-token"))
        })
        .unwrap();
        assert_eq!(env_endpoint.api_key.as_deref(), Some("env-token"));

        let blank_base = WorkflowArguments(BTreeMap::from([(
            "params.base_url".into(),
            ValueRef::from("   "),
        )]));
        assert!(resolve_model_list_endpoint(&schema, &ports, &blank_base, |_| None).is_none());

        let no_default_schema: WorkflowInputSchema = serde_json::from_value(json!({
            "inputs": {
                "params": {
                    "model": {"type": "string", "required": true, "binding": {"secret": false}},
                    "base_url": {"type": "string", "required": true, "binding": {"secret": false}}
                }
            },
            "stdin": {}
        }))
        .unwrap();
        assert!(
            resolve_model_list_endpoint(&no_default_schema, &ports, &empty, |_| None).is_none()
        );

        let saved_with_secret = ProfilePreferences {
            parameters: BTreeMap::from([
                ("params.api_key".into(), json!("leaked-in-file")),
                ("params.temperature".into(), json!("not-a-number")),
                ("params.base_url".into(), json!("https://api.openai.com/v1")),
            ]),
        };
        let merged =
            merge_parameters(&schema, Some(&ports), &saved_with_secret, &empty, |_| None).unwrap();
        assert!(!merged.effective.0.contains_key("params.api_key"));
        assert!(!merged.effective.0.contains_key("params.temperature"));
        assert!(merged.needs_default_model);
    }

    #[test]
    fn validate_launch_parameters_defers_only_discoverable_model_port() {
        let schema: WorkflowInputSchema = serde_json::from_value(json!({
            "inputs": {
                "params": {
                    "model": {
                        "type": "string",
                        "required": true,
                        "binding": {"secret": false}
                    },
                    "base_url": {
                        "type": "string",
                        "required": true,
                        "binding": {"secret": false}
                    }
                }
            },
            "stdin": {}
        }))
        .unwrap();
        let ports = ProviderParameterPorts {
            model: "params.model".into(),
            base_url: "params.base_url".into(),
            api_key: None,
        };
        let only_base_url = WorkflowArguments(BTreeMap::from([(
            "params.base_url".into(),
            ValueRef::from("http://127.0.0.1:8080/v1"),
        )]));
        assert!(validate_launch_parameters(&schema, Some(&ports), &only_base_url).is_ok());
        assert!(validate_launch_parameters(&schema, None, &only_base_url).is_err());
        assert!(
            validate_launch_parameters(&schema, Some(&ports), &WorkflowArguments::default())
                .is_err()
        );
    }

    #[test]
    fn model_selection_discovers_lists_switches_and_persists_models_natively() {
        let server = Server::start_models(vec![
            Reply::models(json!([
                {"id": "first-model", "context_window": 131072},
                {"id": "second-model", "context_window": 65536}
            ])),
            Reply::models(json!([
                {"id": "first-model", "context_window": 131072},
                {"id": "second-model", "context_window": 65536}
            ])),
            Reply::models(json!([
                {"id": "first-model", "context_window": 131072},
                {"id": "second-model", "context_window": 65536}
            ])),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chat.sqlite");
        let contract = model_selectable_contract();
        let explicit = WorkflowArguments(BTreeMap::from([
            (
                "params.base_url".into(),
                ValueRef::from(server.base_url.as_str()),
            ),
            ("params.api_key".into(), ValueRef::from("sk-test")),
        ]));

        let mut selection =
            ModelSelection::prepare(&db_path, "default", &contract, &explicit).unwrap();
        assert!(selection.supports_model_selection());
        assert!(selection.needs_default_model());
        assert!(!preferences_path(&db_path).exists());

        selection.commit_startup_preferences().unwrap();
        assert!(preferences_path(&db_path).exists());

        let discovered = selection.discover_default_model();
        assert_eq!(discovered.model.as_deref(), Some("first-model"));
        assert_eq!(discovered.context_window, Some(131_072));
        assert_eq!(discovered.warning, None);
        assert!(!selection.needs_default_model());
        assert_eq!(selection.display_model().as_deref(), Some("first-model"));

        let listed = selection.list_models().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[1].id, "second-model");

        let selected = selection.select_model("second-model").unwrap();
        assert_eq!(selected.model.as_deref(), Some("second-model"));
        assert_eq!(selected.context_window, Some(65_536));
        assert_eq!(selected.warning, None);
        assert!(selection.select_model("   ").is_err());

        // Re-preparing without explicit parameters reuses saved base_url and second-model,
        // and probe_context_window discovers second-model's window via llm.model_info.
        let mut reopened = ModelSelection::prepare(
            &db_path,
            "default",
            &contract,
            &WorkflowArguments::default(),
        )
        .unwrap();
        assert!(!reopened.needs_default_model());
        assert_eq!(reopened.display_model().as_deref(), Some("second-model"));
        assert_eq!(reopened.cached_context_window("second-model"), None);
        assert_eq!(reopened.probe_context_window("second-model"), Some(65_536));
        assert_eq!(
            reopened.cached_context_window("second-model"),
            Some(Some(65_536))
        );
        // Subsequent probes reuse the cached value without hitting the server again.
        assert_eq!(reopened.probe_context_window("second-model"), Some(65_536));
        let shared = reopened.context_windows();
        assert_eq!(
            shared.get(&server.base_url, "second-model"),
            Some(Some(65_536))
        );
        assert_eq!(shared.get("/", "second-model"), None);
        shared.record("/", "ignored", Some(1_024));
        assert_eq!(shared.get("/", "ignored"), None);
        shared.record("http://example.test/", "zero-model", Some(0));
        assert_eq!(shared.get("http://example.test", "zero-model"), Some(None));
    }

    #[test]
    fn model_selection_cancellation_aborts_default_discovery_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chat.sqlite");
        let contract = model_selectable_contract();
        let explicit = WorkflowArguments(BTreeMap::from([(
            "params.base_url".into(),
            ValueRef::from("http://127.0.0.1:1/v1"),
        )]));
        let mut selection =
            ModelSelection::prepare(&db_path, "default", &contract, &explicit).unwrap();
        assert_eq!(selection.display_model().as_deref(), Some("fallback-model"));

        let cancel = selection.cancellation();
        let active = cancel.begin_query();
        cancel.cancel_active();
        assert!(active.failure().is_some());
        cancel.finish_query();

        cancel.cancel();
        assert!(cancel.is_cancelled());
        let outcome = selection.discover_default_model();
        assert_eq!(outcome.model, None);
        assert_eq!(outcome.warning, None);
        assert_eq!(selection.probe_context_window("first-model"), None);
    }

    #[test]
    fn model_selection_handles_preference_load_and_commit_failures() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chat.sqlite");
        let prefs = preferences_path(&db_path);
        let contract = model_selectable_contract();

        std::fs::write(&prefs, "{not-valid-json").unwrap();
        assert!(matches!(
            ModelSelection::prepare(
                &db_path,
                "default",
                &contract,
                &WorkflowArguments::default()
            ),
            Err(ChatError::PreferencesJson { .. })
        ));
        std::fs::remove_file(&prefs).unwrap();

        let explicit = WorkflowArguments(BTreeMap::from([(
            "params.base_url".into(),
            ValueRef::from("http://127.0.0.1:8080/v1"),
        )]));
        let mut selection =
            ModelSelection::prepare(&db_path, "default", &contract, &explicit).unwrap();
        std::fs::create_dir(&prefs).unwrap();
        assert!(matches!(
            selection.commit_startup_preferences(),
            Err(ChatError::Io { .. })
        ));

        // Failed commit retains pending startup preferences so a retry persists them.
        std::fs::remove_dir(&prefs).unwrap();
        selection.commit_startup_preferences().unwrap();
        assert!(prefs.is_file());
    }
}
