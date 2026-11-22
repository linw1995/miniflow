//! Persisted non-secret startup preferences for interactive chat sessions.

use super::{ChatRunError, IoSnafu, PreferencesJsonSnafu, PreferencesSnafu};
use mf_chat::contract::ProviderParameterPorts;
use mf_runtime::{ValueRef, WorkflowArguments, WorkflowInputSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use snafu::{ResultExt, ensure};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const PREFERENCES_VERSION: u32 = 1;
const MAX_PREFERENCES_BYTES: usize = 1024 * 1024;

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
pub struct ProfilePreferences {
    #[serde(default)]
    pub parameters: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
pub struct MergedParameters {
    pub effective: WorkflowArguments,
    pub updated_profile: ProfilePreferences,
    pub preferences_changed: bool,
    pub needs_default_model: bool,
}

pub fn preferences_path(database_path: &Path) -> PathBuf {
    let mut name = database_path.as_os_str().to_os_string();
    name.push(".preferences.json");
    PathBuf::from(name)
}

fn load_store(path: &Path) -> Result<ChatPreferencesStore, ChatRunError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ChatPreferencesStore::default());
        }
        Err(error) => {
            return Err(error).context(IoSnafu {
                operation: "open chat preferences",
            });
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_PREFERENCES_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .context(IoSnafu {
            operation: "read chat preferences",
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

fn save_store(path: &Path, store: &ChatPreferencesStore) -> Result<(), ChatRunError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut bytes = serde_json::to_vec_pretty(store)?;
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
        })?;
    temp.write_all(&bytes).context(IoSnafu {
        operation: "write chat preferences",
    })?;
    temp.as_file().sync_all().context(IoSnafu {
        operation: "sync chat preferences",
    })?;
    temp.persist(path)
        .map_err(|error| error.error)
        .context(IoSnafu {
            operation: "replace chat preferences",
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
) -> Result<MergedParameters, ChatRunError> {
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
            updated_profile
                .parameters
                .insert(name.clone(), serde_json::to_value(value)?);
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
) -> Result<(), mf_runtime::WorkflowInputError> {
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

pub fn resolve_launch_preferences(
    schema: &WorkflowInputSchema,
    provider_ports: Option<&ProviderParameterPorts>,
    prefs_path: &Path,
    profile: &str,
    explicit: &WorkflowArguments,
) -> Result<MergedParameters, ChatRunError> {
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

pub fn resolve_model_list_query(
    schema: &WorkflowInputSchema,
    provider_ports: &ProviderParameterPorts,
    parameters: &WorkflowArguments,
    timeout_ms: u64,
    mut environment: impl FnMut(&str) -> Option<OsString>,
) -> Option<mfn_llm::ModelListQuery> {
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
    Some(mfn_llm::ModelListQuery {
        base_url,
        api_key,
        timeout_ms,
    })
}

pub fn resolve_display_model(
    schema: &WorkflowInputSchema,
    provider_ports: &ProviderParameterPorts,
    parameters: &WorkflowArguments,
    environment: impl FnMut(&str) -> Option<OsString>,
) -> Option<String> {
    resolve_string_parameter(schema, parameters, &provider_ports.model, environment)
        .filter(|value| !value.trim().is_empty())
}

pub fn persist_profile(
    path: &Path,
    profile: &str,
    preferences: ProfilePreferences,
) -> Result<(), ChatRunError> {
    let mut store = load_store(path)?;
    store.profiles.insert(profile.to_owned(), preferences);
    save_store(path, &store)
}

pub fn persist_selected_model(
    path: &Path,
    profile: &str,
    schema: &WorkflowInputSchema,
    model_port: &str,
    model: &str,
) -> Result<(), ChatRunError> {
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

        // Restart without explicit CLI args reuses both base_url and model.
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
        // Unknown entries from other chatflows stay preserved in the profile store.
        assert!(
            reused
                .updated_profile
                .parameters
                .contains_key("params.unknown")
        );

        // Explicit new base_url without explicit model drops stale saved model.
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
            Err(ChatRunError::PreferencesJson { .. })
        ));

        std::fs::write(&path, json!({"version": 99, "profiles": {}}).to_string()).unwrap();
        assert!(matches!(
            load_store(&path),
            Err(ChatRunError::Preferences { .. })
        ));

        // Oversized files and non-file paths are rejected cleanly.
        std::fs::write(&path, vec![b' '; MAX_PREFERENCES_BYTES + 1]).unwrap();
        assert!(matches!(
            load_store(&path),
            Err(ChatRunError::Preferences { .. })
        ));
        let dir_path = dir.path().join("not-a-file");
        std::fs::create_dir(&dir_path).unwrap();
        assert!(matches!(
            load_store(&dir_path),
            Err(ChatRunError::Io { .. })
        ));

        // Serialized stores exceeding the size limit are refused before writing.
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
            Err(ChatRunError::Preferences { .. })
        ));

        // Unchanged model and non-persistable ports are no-ops.
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

        // Falls back to schema defaults when no explicit value or env var exists.
        assert_eq!(
            resolve_display_model(&schema, &ports, &empty, |_| None).as_deref(),
            Some("your-model")
        );
        let default_query =
            resolve_model_list_query(&schema, &ports, &empty, 5_000, |_| None).unwrap();
        assert_eq!(default_query.base_url, "https://api.openai.com/v1");
        assert_eq!(default_query.api_key, None);
        assert_eq!(default_query.timeout_ms, 5_000);

        // Picks up env-bound api_key and ignores empty base_url.
        let env_query = resolve_model_list_query(&schema, &ports, &empty, 5_000, |name| {
            (name == "OPENAI_API_KEY").then(|| OsString::from("env-token"))
        })
        .unwrap();
        assert_eq!(env_query.api_key.as_deref(), Some("env-token"));

        let blank_base = WorkflowArguments(BTreeMap::from([(
            "params.base_url".into(),
            ValueRef::from("   "),
        )]));
        assert!(resolve_model_list_query(&schema, &ports, &blank_base, 5_000, |_| None).is_none());

        // Saved secret or ill-typed entries are ignored during merge.
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
}
