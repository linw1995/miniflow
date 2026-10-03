use crate::{FlowNode, Inputs, ValueType};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;
use snafu::Snafu;
use std::{collections::BTreeMap, fmt};

pub const MAX_WORKFLOW_INPUT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputResource {
    Stdin,
    Channel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInput {
    #[serde(rename = "type")]
    pub value_type: ValueType,
    pub required: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputSchema {
    pub inputs: BTreeMap<String, BTreeMap<String, WorkflowInput>>,
    pub resources: BTreeMap<String, Vec<InputResource>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct WorkflowArguments(pub BTreeMap<String, Inputs>);

#[derive(Debug, Snafu)]
pub enum WorkflowInputError {
    #[snafu(display("invalid workflow arguments: {source}"))]
    Json { source: serde_json::Error },
    #[snafu(display("workflow arguments exceed the {limit}-byte limit"))]
    TooLarge { limit: usize },
    #[snafu(display("workflow input `{path}`: {message}"))]
    Invalid { path: String, message: String },
}

fn invalid(path: String, message: impl Into<String>) -> WorkflowInputError {
    WorkflowInputError::Invalid {
        path,
        message: message.into(),
    }
}

fn pointer(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

impl WorkflowArguments {
    pub fn from_json(bytes: &[u8]) -> Result<Self, WorkflowInputError> {
        if bytes.len() > MAX_WORKFLOW_INPUT_BYTES {
            return Err(WorkflowInputError::TooLarge {
                limit: MAX_WORKFLOW_INPUT_BYTES,
            });
        }
        let value: UniqueValue =
            serde_json::from_slice(bytes).map_err(|source| WorkflowInputError::Json { source })?;
        Self::try_from(value.0)
    }
}

impl TryFrom<Value> for WorkflowArguments {
    type Error = WorkflowInputError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let Value::Object(nodes) = value else {
            return Err(invalid(
                String::new(),
                "expected an object keyed by initial node ID",
            ));
        };
        let mut values = BTreeMap::new();
        for (node, value) in nodes {
            let Value::Object(ports) = value else {
                return Err(invalid(
                    pointer("", &node),
                    "expected an object keyed by input port",
                ));
            };
            values.insert(
                node,
                ports
                    .into_iter()
                    .map(|(key, value)| (key, value.into()))
                    .collect(),
            );
        }
        Ok(Self(values))
    }
}

impl WorkflowInputSchema {
    pub fn from_nodes<'a, N: 'a>(
        nodes: impl IntoIterator<Item = (&'a FlowNode<N>, bool)>,
    ) -> Result<Self, WorkflowInputError> {
        let mut schema = Self::default();
        let mut stdin_owner = None;
        for (node, initial) in nodes {
            let id = node.definition_id.as_str();
            if initial {
                let mut ports = BTreeMap::new();
                for port in &node.metadata.ports.inputs {
                    let path = pointer(&pointer("", id), &port.name);
                    port.value_type
                        .check_depth()
                        .map_err(|error| invalid(path.clone(), error.to_string()))?;
                    if port.name.is_empty()
                        || ports
                            .insert(
                                port.name.to_string(),
                                WorkflowInput {
                                    value_type: port.value_type.clone(),
                                    required: port.required,
                                },
                            )
                            .is_some()
                    {
                        return Err(invalid(path, "empty or duplicate input port"));
                    }
                }
                if schema.inputs.insert(id.into(), ports).is_some() {
                    return Err(invalid(pointer("", id), "duplicate initial node"));
                }
            }
            let mut resources = std::collections::BTreeSet::new();
            for resource in &node.metadata.resources {
                if !initial {
                    return Err(invalid(
                        pointer("", id),
                        "input resources require an initial node",
                    ));
                }
                if !resources.insert(*resource) {
                    return Err(invalid(pointer("", id), "duplicate input resource"));
                }
                if *resource == InputResource::Stdin
                    && let Some(previous) = stdin_owner.replace(id.to_owned())
                {
                    return Err(invalid(
                        pointer("", id),
                        format!("stdin is already required by node `{previous}`"),
                    ));
                }
            }
            if !resources.is_empty() {
                schema
                    .resources
                    .insert(id.into(), resources.into_iter().collect());
            }
        }
        Ok(schema)
    }

    pub fn validate(&self, arguments: &WorkflowArguments) -> Result<(), WorkflowInputError> {
        for (node, values) in &arguments.0 {
            let node_path = pointer("", node);
            let ports = self
                .inputs
                .get(node)
                .ok_or_else(|| invalid(node_path.clone(), "unknown initial node"))?;
            for (port, value) in values {
                let path = pointer(&node_path, port);
                let input = ports
                    .get(port)
                    .ok_or_else(|| invalid(path.clone(), "unknown input port"))?;
                input.value_type.validate_shared(value).map_err(|error| {
                    invalid(
                        format!("{path}{}", error.path),
                        format!("expected {}, found {}", error.expected, error.actual),
                    )
                })?;
            }
        }
        for (node, ports) in &self.inputs {
            for (port, input) in ports {
                if input.required
                    && !arguments
                        .0
                        .get(node)
                        .is_some_and(|values| values.contains_key(port))
                {
                    return Err(invalid(
                        pointer(&pointer("", node), port),
                        "required input was not provided",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn validate_resources(
        &self,
        mut available: impl FnMut(&str, InputResource) -> bool,
    ) -> Result<(), WorkflowInputError> {
        for (node, requirements) in &self.resources {
            for &resource in requirements {
                if !available(node, resource) {
                    return Err(invalid(
                        pointer("", node),
                        format!("required input resource {resource:?} is unavailable"),
                    ));
                }
            }
        }
        Ok(())
    }
}

struct UniqueValue(Value);

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
                        return Err(de::Error::custom(format!("duplicate object key `{key}`")));
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
