use crate::{FlowNode, Inputs, ValueType};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;
use snafu::{ResultExt, Snafu};
use std::{collections::BTreeMap, fmt};

pub const MAX_WORKFLOW_INPUT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowInterfaceVersion {
    #[serde(rename = "2026-10-03")]
    V2026_10_03,
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
        let value: UniqueValue = serde_json::from_slice(bytes).context(JsonSnafu)?;
        serde_json::from_value(value.0).context(JsonSnafu)
    }

    pub fn validate_for_description(
        &self,
        description: &mf_telemetry::description::WorkflowDescription,
    ) -> Result<(), WorkflowInputError> {
        use std::collections::BTreeSet;
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
        for (node, ports) in &self.schema.inputs {
            for (name, input) in ports {
                if name.is_empty() {
                    return Err(invalid(pointer("", node), "empty input port name"));
                }
                input.value_type.check_depth().context(TypeDepthSnafu {
                    path: pointer(&pointer("", node), name),
                })?;
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
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputSchema {
    pub inputs: BTreeMap<String, BTreeMap<String, WorkflowInput>>,
    pub stdin: BTreeMap<String, StdinRequirement>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct WorkflowArguments(pub BTreeMap<String, Inputs>);

#[derive(Debug, Snafu)]
pub enum WorkflowInputError {
    #[snafu(display("invalid workflow arguments: {source}"), visibility(pub))]
    Json { source: serde_json::Error },
    #[snafu(
        display("workflow arguments exceed the {limit}-byte limit"),
        visibility(pub)
    )]
    TooLarge { limit: usize },
    #[snafu(display("workflow input `{path}`: {message}"))]
    Invalid { path: String, message: String },
    #[snafu(display("workflow input `{path}`: {source}"))]
    TypeDepth {
        path: String,
        source: crate::TypeDepthError,
    },
    #[snafu(display(
        "workflow input `{}`: expected {}, found {}",
        format!("{path}{}", source.path),
        source.expected,
        source.actual
    ))]
    TypeMismatch {
        path: String,
        source: crate::TypeMismatch,
    },
}

fn invalid(path: String, message: impl Into<String>) -> WorkflowInputError {
    let message: String = message.into();
    InvalidSnafu { path, message }.build()
}

fn pointer(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

impl WorkflowArguments {
    pub fn from_json(bytes: &[u8]) -> Result<Self, WorkflowInputError> {
        if bytes.len() > MAX_WORKFLOW_INPUT_BYTES {
            return TooLargeSnafu {
                limit: MAX_WORKFLOW_INPUT_BYTES,
            }
            .fail();
        }
        let value: UniqueValue = serde_json::from_slice(bytes).context(JsonSnafu)?;
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
        mut input_bound: impl FnMut(&str, &str) -> bool,
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
                        .context(TypeDepthSnafu { path: path.clone() })?;
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
            if let Some(requirement) = &node.metadata.stdin {
                if let StdinRequirement::UnlessInput(input) = requirement {
                    if !node
                        .metadata
                        .ports
                        .inputs
                        .iter()
                        .any(|port| port.name == *input)
                    {
                        return Err(invalid(
                            pointer("", id),
                            "stdin condition names an unknown input",
                        ));
                    }
                    if !initial && input_bound(id, input) {
                        continue;
                    }
                }
                if !initial {
                    return Err(invalid(pointer("", id), "stdin requires an initial node"));
                }
                if *requirement == StdinRequirement::Always
                    && let Some(previous) = stdin_owner.replace(id.to_owned())
                {
                    return Err(invalid(
                        pointer("", id),
                        format!("stdin is already required by node `{previous}`"),
                    ));
                }
                schema.stdin.insert(id.into(), requirement.clone());
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
                input
                    .value_type
                    .validate_shared(value)
                    .context(TypeMismatchSnafu { path })?;
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
                    .get(node)
                    .is_some_and(|values| values.contains_key(input)),
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
