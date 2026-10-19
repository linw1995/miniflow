use crate::{Inputs, ValueType};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;
use snafu::{ResultExt, Snafu, ensure};
use std::{collections::BTreeMap, fmt};

pub const MAX_WORKFLOW_INPUT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowInterfaceVersion {
    #[serde(rename = "2026-10-11")]
    V2026_10_11,
    #[serde(rename = "2026-10-16")]
    V2026_10_16,
}

impl WorkflowInterfaceVersion {
    pub fn for_schema(schema: &WorkflowInputSchema) -> Self {
        if schema
            .inputs
            .values()
            .flat_map(|ports| ports.values())
            .any(|input| !input.binding.is_empty())
        {
            Self::V2026_10_16
        } else {
            Self::V2026_10_11
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInterface {
    pub version: WorkflowInterfaceVersion,
    pub workflow_id: mf_telemetry::identity::WorkflowId,
    pub schema: WorkflowInputSchema,
}

impl WorkflowInterface {
    pub fn from_json(bytes: &[u8]) -> Result<Self, WorkflowInputError> {
        if bytes.len() > mf_telemetry::description::MAX_DESCRIPTION_BYTES {
            return TooLargeSnafu {
                limit: mf_telemetry::description::MAX_DESCRIPTION_BYTES,
            }
            .fail();
        }
        from_unique_json(bytes).context(JsonSnafu)
    }

    pub fn validate_for_description(
        &self,
        description: &mf_telemetry::description::WorkflowDescription,
    ) -> Result<(), WorkflowInputError> {
        use std::collections::BTreeSet;
        if self.version == WorkflowInterfaceVersion::V2026_10_11
            && WorkflowInterfaceVersion::for_schema(&self.schema) != self.version
        {
            return Err(invalid(
                String::new(),
                "input bindings require interface version 2026-10-16",
            ));
        }
        if self.workflow_id != description.workflow_id {
            return Err(invalid(
                String::new(),
                "graph and interface workflow identities disagree",
            ));
        }
        let incoming: BTreeSet<_> = description
            .data_edges
            .iter()
            .map(|edge| edge.to_node.as_str())
            .chain(
                description
                    .control_edges
                    .iter()
                    .map(|edge| edge.to_node.as_str()),
            )
            .collect();
        let roots: BTreeSet<_> = description
            .nodes
            .iter()
            .filter(|node| !incoming.contains(node.id.as_str()))
            .map(|node| node.id.as_str())
            .collect();
        let declared: BTreeSet<_> = self.schema.inputs.keys().map(String::as_str).collect();
        if description.execution.is_some() && roots != declared {
            return Err(invalid(
                String::new(),
                "interface must describe every initial node exactly once",
            ));
        }
        if !declared.is_subset(&roots) {
            return Err(invalid(
                String::new(),
                "interface names a noninitial or unknown node",
            ));
        }
        let mut stdin = None;
        let mut names = std::collections::BTreeSet::new();
        for (node, ports) in &self.schema.inputs {
            for (name, input) in ports {
                if name.is_empty() {
                    return Err(invalid(pointer("", node), "empty input port name"));
                }
                let qualified = crate::output_id(node, name);
                if !names.insert(qualified.clone()) {
                    return Err(invalid(
                        pointer("", &qualified),
                        "ambiguous workflow input name",
                    ));
                }
                input.value_type.check_depth().context(TypeDepthSnafu {
                    path: pointer(&pointer("", node), name),
                })?;
                input.binding.validate(&qualified, &input.value_type)?;
            }
        }
        for (node, requirement) in &self.schema.stdin {
            if !declared.contains(node.as_str()) {
                return Err(invalid(
                    pointer("", node),
                    "stdin owner is not an initial node",
                ));
            }
            if let StdinRequirement::UnlessInput(input) = requirement
                && !self.schema.inputs[node].contains_key(input)
            {
                return Err(invalid(
                    pointer("", node),
                    "stdin condition names an unknown input",
                ));
            }
            if *requirement == StdinRequirement::Always && stdin.replace(node).is_some() {
                return Err(invalid(pointer("", node), "stdin has multiple owners"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StdinRequirement {
    Always,
    UnlessInput(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInput {
    #[serde(rename = "type")]
    pub value_type: ValueType,
    pub required: bool,
    #[serde(default, skip_serializing_if = "InputBinding::is_empty")]
    pub binding: InputBinding,
}

/// Startup sources are resolved before execution and before input observation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputBinding {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    #[serde(
        default,
        deserialize_with = "present_default",
        skip_serializing_if = "Option::is_none"
    )]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub secret: bool,
}

fn present_default<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

fn is_false(value: &bool) -> bool {
    !value
}

impl InputBinding {
    pub fn is_empty(&self) -> bool {
        self.env.is_none() && self.default.is_none() && !self.secret
    }

    pub fn validate(&self, name: &str, value_type: &ValueType) -> Result<(), WorkflowInputError> {
        if let Some(env) = &self.env
            && (env.is_empty() || env.contains(['=', '\0']))
        {
            return Err(invalid(
                pointer("", name),
                "invalid environment variable name",
            ));
        }
        if let Some(value) = &self.default {
            value_type
                .validate_shared(&value.clone().into())
                .context(TypeMismatchSnafu {
                    path: pointer("", name),
                })?;
        }
        Ok(())
    }
}

impl WorkflowInput {
    /// Uses the same text representation for named arguments and environment values.
    pub fn parse_text(&self, text: &str) -> Result<crate::ValueRef, serde_json::Error> {
        let value = match &self.value_type {
            ValueType::String => crate::ValueRef::from(text),
            ValueType::Nullable(inner) if **inner == ValueType::String && text != "null" => {
                crate::ValueRef::from(text)
            }
            _ => from_unique_json::<Value>(text.as_bytes())?.into(),
        };
        Ok(if self.binding.secret {
            value.into_secret()
        } else {
            value
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputSchema {
    pub inputs: BTreeMap<String, BTreeMap<String, WorkflowInput>>,
    pub stdin: BTreeMap<String, StdinRequirement>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct WorkflowArguments(pub Inputs);

#[derive(Debug, Snafu)]
pub enum WorkflowInputError {
    #[snafu(display("invalid workflow arguments: {source}"), visibility(pub))]
    Json { source: serde_json::Error },
    #[snafu(
        display("workflow input `{path}` requires valid JSON"),
        visibility(pub(super))
    )]
    TextValue {
        path: String,
        source: serde_json::Error,
    },
    #[snafu(
        display("workflow arguments exceed the {limit}-byte limit"),
        visibility(pub)
    )]
    TooLarge { limit: usize },
    #[snafu(display("workflow input `{path}`: {message}"), visibility(pub))]
    Invalid { path: String, message: String },
    #[snafu(display("workflow input `{path}`: {source}"), visibility(pub))]
    TypeDepth {
        path: String,
        source: crate::TypeDepthError,
    },
    #[snafu(display(
        "workflow input `{}`: expected {}, found {}",
        format!("{path}{}", source.path),
        source.expected,
        source.actual
    ), visibility(pub(super)))]
    TypeMismatch {
        path: String,
        source: crate::TypeMismatch,
    },
}

fn invalid(path: String, message: impl Into<String>) -> WorkflowInputError {
    let message: String = message.into();
    InvalidSnafu { path, message }.build()
}

pub(super) fn pointer(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

impl WorkflowArguments {
    pub(super) fn check_size(size: usize) -> Result<(), WorkflowInputError> {
        ensure!(
            size <= MAX_WORKFLOW_INPUT_BYTES,
            TooLargeSnafu {
                limit: MAX_WORKFLOW_INPUT_BYTES,
            }
        );
        Ok(())
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, WorkflowInputError> {
        Self::check_size(bytes.len())?;
        let value: UniqueValue = serde_json::from_slice(bytes).context(JsonSnafu)?;
        Self::try_from(value.0)
    }
}

impl TryFrom<Value> for WorkflowArguments {
    type Error = WorkflowInputError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let Value::Object(values) = value else {
            return Err(invalid(
                String::new(),
                "expected an object keyed by workflow input name",
            ));
        };
        let values = values
            .into_iter()
            .map(|(name, value)| (name, value.into()))
            .collect();
        Ok(Self(values))
    }
}

impl WorkflowInputSchema {
    /// Checks supplied values and declarations without resolving external sources.
    pub fn validate(&self, arguments: &WorkflowArguments) -> Result<(), WorkflowInputError> {
        for (name, value) in &arguments.0 {
            let path = pointer("", name);
            let input = self
                .parameter(name)
                .ok_or_else(|| invalid(path.clone(), "unknown workflow input"))?;
            input
                .value_type
                .validate_shared(value)
                .context(TypeMismatchSnafu { path })?;
        }
        for (node, ports) in &self.inputs {
            for (port, input) in ports {
                let name = crate::output_id(node, port);
                input.binding.validate(&name, &input.value_type)?;
                if input.required
                    && input.binding.env.is_none()
                    && input.binding.default.is_none()
                    && !arguments.0.contains_key(name.as_str())
                {
                    return Err(invalid(
                        pointer("", &name),
                        "required input was not provided",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Does not read sources; callers may inspect or forward arguments before execution.
    pub fn mark_secrets(&self, arguments: &mut WorkflowArguments) {
        for (node, ports) in &self.inputs {
            for (port, input) in ports {
                if input.binding.secret
                    && let Some(value) = arguments.0.get_mut(&crate::output_id(node, port))
                {
                    *value = value.clone().into_secret();
                }
            }
        }
    }

    pub fn parameter(&self, name: &str) -> Option<&WorkflowInput> {
        self.inputs.iter().find_map(|(node, ports)| {
            ports
                .iter()
                .find_map(|(port, input)| (crate::output_id(node, port) == name).then_some(input))
        })
    }

    pub fn stdin_owner<'a>(
        &'a self,
        arguments: &WorkflowArguments,
    ) -> Result<Option<&'a str>, WorkflowInputError> {
        let mut owner = None;
        for (node, requirement) in &self.stdin {
            let required = match requirement {
                StdinRequirement::Always => true,
                StdinRequirement::UnlessInput(input) => !arguments
                    .0
                    .contains_key(crate::output_id(node, input).as_str()),
            };
            if required && let Some(previous) = owner.replace(node.as_str()) {
                return Err(invalid(
                    pointer("", node),
                    format!("stdin is already required by node `{previous}`"),
                ));
            }
        }
        Ok(owner)
    }

    pub fn validate_stdin(
        &self,
        arguments: &WorkflowArguments,
        stdin_available: bool,
    ) -> Result<(), WorkflowInputError> {
        if let Some(node) = self.stdin_owner(arguments)?
            && !stdin_available
        {
            return Err(invalid(
                pointer("", node),
                "required stdin resource is unavailable",
            ));
        }
        Ok(())
    }
}

pub struct UniqueValue(pub Value);

/// Decodes JSON with unique object keys; callers bound the input before decoding.
pub fn from_unique_json<T: de::DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_json::Error> {
    let value: UniqueValue = serde_json::from_slice(bytes)?;
    serde_json::from_value(value.0)
}

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = UniqueValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| UniqueValue(Value::Number(number)))
                    .ok_or_else(|| E::custom("nonfinite JSON number"))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(UniqueValue(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom("duplicate object key"));
                    }
                    let UniqueValue(value) = map.next_value()?;
                    values.insert(key, value);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}
