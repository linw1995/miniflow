use serde::de::DeserializeOwned;
use serde_json::Value;
use snafu::{IntoError, Snafu};
use std::borrow::Cow;
use std::error::Error;
use std::fmt;

pub type Inputs = std::collections::BTreeMap<String, crate::ValueRef>;
pub type Outputs = Inputs;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueType {
    Any,
    Null,
    Boolean,
    Number,
    Int64,
    Float64,
    String,
    Array,
    Object,
    List(Box<ValueType>),
    Map(Box<ValueType>),
}

impl serde::Serialize for ValueType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let scalar = match self {
            Self::Any => "any",
            Self::Null => "null",
            Self::Boolean => "bool",
            Self::Number => "number",
            Self::Int64 => "int",
            Self::Float64 => "double",
            Self::String => "string",
            Self::Array => "array",
            Self::Object => "object",
            Self::List(inner) | Self::Map(inner) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(
                    if matches!(self, Self::List(_)) {
                        "list"
                    } else {
                        "map"
                    },
                    inner,
                )?;
                return map.end();
            }
        };
        serializer.serialize_str(scalar)
    }
}

impl<'de> serde::Deserialize<'de> for ValueType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <Value as serde::Deserialize>::deserialize(deserializer)?;
        Self::parse_descriptor(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeCompatibility {
    Static,
    Checked,
    Incompatible,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeMismatch {
    pub path: String,
    pub expected: ValueType,
    pub actual: &'static str,
}

impl fmt::Display for TypeMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "path `{}`: expected {}, found {}",
            self.path, self.expected, self.actual
        )
    }
}

impl Error for TypeMismatch {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeDepthError {
    pub depth: usize,
    pub maximum: usize,
}

impl fmt::Display for TypeDepthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "type nesting depth {} exceeds maximum {}",
            self.depth, self.maximum
        )
    }
}

impl Error for TypeDepthError {}

impl ValueType {
    pub const MAX_DEPTH: usize = 16;

    pub fn parse_descriptor(value: &Value) -> Result<Self, String> {
        fn parse(value: &Value, depth: usize) -> Result<ValueType, String> {
            if depth > ValueType::MAX_DEPTH {
                return Err(format!(
                    "type nesting depth exceeds {}",
                    ValueType::MAX_DEPTH
                ));
            }
            match value {
                Value::String(name) => match name.as_str() {
                    "any" => Ok(ValueType::Any),
                    "null" => Ok(ValueType::Null),
                    "bool" => Ok(ValueType::Boolean),
                    "number" => Ok(ValueType::Number),
                    "int" => Ok(ValueType::Int64),
                    "double" => Ok(ValueType::Float64),
                    "string" => Ok(ValueType::String),
                    "array" => Ok(ValueType::Array),
                    "object" => Ok(ValueType::Object),
                    _ => Err(format!("unsupported type `{name}`")),
                },
                Value::Object(fields) if fields.len() == 1 => {
                    let (kind, inner) = fields.iter().next().unwrap();
                    match kind.as_str() {
                        "list" => Ok(ValueType::List(Box::new(parse(inner, depth + 1)?))),
                        "map" => Ok(ValueType::Map(Box::new(parse(inner, depth + 1)?))),
                        _ => Err(format!("unsupported type constructor `{kind}`")),
                    }
                }
                _ => Err("type must be a scalar, list, or map descriptor".into()),
            }
        }
        parse(value, 1)
    }

    pub fn is_concrete(&self) -> bool {
        match self {
            Self::Null | Self::Boolean | Self::Int64 | Self::Float64 | Self::String => true,
            Self::List(inner) | Self::Map(inner) => inner.is_concrete(),
            Self::Any | Self::Number | Self::Array | Self::Object => false,
        }
    }

    pub fn infer_json(value: &Value) -> Self {
        Self::infer_view(value, 1)
    }
    pub fn infer_shared(value: &crate::ValueRef) -> Self {
        Self::infer_view(value, 1)
    }

    fn infer_view<V: crate::value::JsonView>(value: &V, depth: usize) -> Self {
        match value.shape() {
            "null" => Self::Null,
            "boolean" => Self::Boolean,
            "number" => match value.number().unwrap() {
                number if number.is_f64() => Self::Float64,
                number if number.as_i64().is_some() => Self::Int64,
                _ => Self::Number,
            },
            "string" => Self::String,
            "array" => {
                let mut items = value.array_items();
                if depth < Self::MAX_DEPTH
                    && let Some(first) = items.next()
                {
                    let element = Self::infer_view(first, depth + 1);
                    if items.all(|item| Self::infer_view(item, depth + 1) == element) {
                        return Self::List(Box::new(element));
                    }
                }
                Self::Array
            }
            "object" => {
                let mut entries = value.object_items();
                if depth < Self::MAX_DEPTH
                    && let Some((_, first)) = entries.next()
                {
                    let element = Self::infer_view(first, depth + 1);
                    if entries.all(|(_, item)| Self::infer_view(item, depth + 1) == element) {
                        return Self::Map(Box::new(element));
                    }
                }
                Self::Object
            }
            _ => unreachable!("JSON values have a known shape"),
        }
    }

    pub fn is_assignable_to(&self, input: &Self) -> bool {
        self.compatibility_with(input) == TypeCompatibility::Static
    }

    pub fn compatibility_with(&self, input: &Self) -> TypeCompatibility {
        use TypeCompatibility::{Checked, Incompatible, Static};

        if input == &Self::Any || self == input {
            return Static;
        }
        match (self, input) {
            (Self::Any, _) => Checked,
            (Self::Int64 | Self::Float64, Self::Number)
            | (Self::List(_), Self::Array)
            | (Self::Map(_), Self::Object) => Static,
            (Self::Number, Self::Int64 | Self::Float64) => Checked,
            (Self::Array, Self::List(inner)) | (Self::Object, Self::Map(inner)) => {
                if inner.as_ref() == &Self::Any {
                    Static
                } else {
                    Checked
                }
            }
            (Self::List(source), Self::List(target)) | (Self::Map(source), Self::Map(target)) => {
                source.compatibility_with(target)
            }
            _ => Incompatible,
        }
    }

    pub fn check_depth(&self) -> Result<(), TypeDepthError> {
        let mut depth = 1;
        let mut current = self;
        while let Self::List(inner) | Self::Map(inner) = current {
            depth += 1;
            if depth > Self::MAX_DEPTH {
                return Err(TypeDepthError {
                    depth,
                    maximum: Self::MAX_DEPTH,
                });
            }
            current = inner;
        }
        Ok(())
    }

    pub fn validate_value(&self, value: &Value) -> Result<(), TypeMismatch> {
        self.validate_at(value)
    }

    pub fn validate_shared(&self, value: &crate::ValueRef) -> Result<(), TypeMismatch> {
        self.validate_at(value)
    }

    fn validate_at<V: crate::value::JsonView>(&self, value: &V) -> Result<(), TypeMismatch> {
        let valid = match (self, value.shape()) {
            (Self::Any, _) => true,
            (Self::Null, "null")
            | (Self::Boolean, "boolean")
            | (Self::Number | Self::Int64 | Self::Float64, "number")
            | (Self::String, "string")
            | (Self::Array, "array")
            | (Self::Object, "object") => match self {
                Self::Int64 => value
                    .number()
                    .is_some_and(|number| !number.is_f64() && number.as_i64().is_some()),
                Self::Float64 => value.number().is_some_and(|number| {
                    number.is_f64() && number.as_f64().is_some_and(f64::is_finite)
                }),
                _ => true,
            },
            (Self::List(inner), "array") => {
                for (index, item) in value.array_items().enumerate() {
                    inner.validate_at(item).map_err(|mut error| {
                        error.path = format!("/{index}{}", error.path);
                        error
                    })?;
                }
                true
            }
            (Self::Map(inner), "object") => {
                for (key, item) in value.object_items() {
                    inner.validate_at(item).map_err(|mut error| {
                        let key = key.replace('~', "~0").replace('/', "~1");
                        error.path = format!("/{key}{}", error.path);
                        error
                    })?;
                }
                true
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(TypeMismatch {
                path: String::new(),
                expected: self.clone(),
                actual: actual_type(value),
            })
        }
    }
}

fn actual_type<V: crate::value::JsonView>(value: &V) -> &'static str {
    if let Some(number) = value.number() {
        if number.is_f64() {
            "float64"
        } else if number.as_i64().is_some() {
            "int64"
        } else {
            "unsigned integer"
        }
    } else {
        value.shape()
    }
}

impl fmt::Display for ValueType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Any => "any",
            Self::Null => "null",
            Self::Boolean => "boolean",
            Self::Number => "number",
            Self::Int64 => "int64",
            Self::Float64 => "float64",
            Self::String => "string",
            Self::Array => "array",
            Self::Object => "object",
            Self::List(inner) => return write!(formatter, "list<{inner}>"),
            Self::Map(inner) => return write!(formatter, "map<{inner}>"),
        };
        formatter.write_str(name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortSpec {
    pub name: Cow<'static, str>,
    pub value_type: ValueType,
    pub required: bool,
}

impl PortSpec {
    pub const fn new(name: &'static str, value_type: ValueType, required: bool) -> Self {
        Self {
            name: Cow::Borrowed(name),
            value_type,
            required,
        }
    }
    pub fn owned(name: impl Into<String>, value_type: ValueType, required: bool) -> Self {
        Self {
            name: Cow::Owned(name.into()),
            value_type,
            required,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodePorts {
    pub inputs: Vec<PortSpec>,
    pub outputs: Vec<PortSpec>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OutputDerivation {
    Literal {
        output: String,
        value: crate::ValueRef,
    },
    ForwardInput {
        output: String,
        input: String,
    },
    CollectInput {
        output: String,
        input: String,
    },
}

impl OutputDerivation {
    pub fn literal(output: impl Into<String>, value: impl Into<crate::ValueRef>) -> Self {
        Self::Literal {
            output: output.into(),
            value: value.into(),
        }
    }

    pub fn forward_input(output: impl Into<String>, input: impl Into<String>) -> Self {
        Self::ForwardInput {
            output: output.into(),
            input: input.into(),
        }
    }

    pub fn output(&self) -> &str {
        match self {
            Self::Literal { output, .. }
            | Self::ForwardInput { output, .. }
            | Self::CollectInput { output, .. } => output,
        }
    }

    pub fn collect_input(output: impl Into<String>, input: impl Into<String>) -> Self {
        Self::CollectInput {
            output: output.into(),
            input: input.into(),
        }
    }
}

#[derive(Debug, Snafu)]
pub enum OutputDerivationError {
    #[snafu(display("node `{node_id}` derivation references unknown output `{output}`"))]
    UnknownOutput { node_id: String, output: String },
    #[snafu(display("node `{node_id}` output `{output}` has more than one derivation"))]
    DuplicateOutput { node_id: String, output: String },
    #[snafu(display("node `{node_id}` output `{output}` forwards unknown input `{input}`"))]
    UnknownInput {
        node_id: String,
        output: String,
        input: String,
    },
    #[snafu(display(
        "node `{node_id}` output `{output}` literal conflicts with its declared type: {source}"
    ))]
    LiteralTypeMismatch {
        node_id: String,
        output: String,
        source: TypeMismatch,
    },
}

impl NodePorts {
    pub fn validate_derivations(
        &self,
        node_id: &str,
        derivations: &[OutputDerivation],
    ) -> Result<(), OutputDerivationError> {
        let mut seen = std::collections::BTreeSet::new();
        for derivation in derivations {
            let output = derivation.output();
            let Some(port) = self.outputs.iter().find(|port| port.name == output) else {
                return Err(OutputDerivationError::UnknownOutput {
                    node_id: node_id.to_owned(),
                    output: output.to_owned(),
                });
            };
            if !seen.insert(output) {
                return Err(OutputDerivationError::DuplicateOutput {
                    node_id: node_id.to_owned(),
                    output: output.to_owned(),
                });
            }
            match derivation {
                OutputDerivation::Literal { value, .. } => {
                    port.value_type.validate_shared(value).map_err(|source| {
                        OutputDerivationError::LiteralTypeMismatch {
                            node_id: node_id.to_owned(),
                            output: output.to_owned(),
                            source,
                        }
                    })?
                }
                OutputDerivation::ForwardInput { input, .. }
                | OutputDerivation::CollectInput { input, .. }
                    if !self.inputs.iter().any(|port| port.name == *input) =>
                {
                    return Err(OutputDerivationError::UnknownInput {
                        node_id: node_id.to_owned(),
                        output: output.to_owned(),
                        input: input.clone(),
                    });
                }
                OutputDerivation::ForwardInput { .. } | OutputDerivation::CollectInput { .. } => {}
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextReference {
    pub output: String,
    pub label: String,
}

impl ContextReference {
    pub fn new(output: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            label: label.into(),
        }
    }
}

pub fn output_id(node: &str, port: &str) -> String {
    format!("{node}.{port}")
}

#[derive(Debug, Snafu)]
pub enum NodeBuildError {
    #[snafu(display("invalid prepared subgraph: {message}"))]
    InvalidSubgraph { message: String },
    #[snafu(display("invalid node configuration: {source}"), context(false))]
    InvalidConfiguration { source: serde_json::Error },
    #[snafu(display("node factory failed: {source}"), visibility(pub))]
    FactoryFailed {
        source: Box<dyn Error + Send + Sync + 'static>,
    },
}

#[derive(Debug, Snafu)]
pub enum NodeExecutionError {
    #[snafu(display("Loop variable `{variable}`: {source}"), visibility(pub))]
    LoopVariableType {
        variable: String,
        #[snafu(source(from(TypeMismatch, Box::new)))]
        source: Box<TypeMismatch>,
    },
    #[snafu(display("node execution failed: {message}"), visibility(pub))]
    ExecutionFailed { message: String },
    #[snafu(display("node plugin failed: {source}"), visibility(pub))]
    PluginFailed {
        source: Box<dyn Error + Send + Sync + 'static>,
    },
}

impl From<Box<dyn Error + Send + Sync>> for NodeBuildError {
    fn from(source: Box<dyn Error + Send + Sync>) -> Self {
        FactoryFailedSnafu.into_error(source)
    }
}

impl From<Box<dyn Error + Send + Sync>> for NodeExecutionError {
    fn from(source: Box<dyn Error + Send + Sync>) -> Self {
        PluginFailedSnafu.into_error(source)
    }
}

impl From<crate::StreamError> for NodeExecutionError {
    fn from(source: crate::StreamError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

impl From<crate::WorkflowRunError> for NodeExecutionError {
    fn from(source: crate::WorkflowRunError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

impl From<crate::WorkerPoolError> for NodeExecutionError {
    fn from(source: crate::WorkerPoolError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

pub fn deserialize_config<T>(value: Value) -> Result<T, NodeBuildError>
where
    T: DeserializeOwned,
{
    Ok(serde_json::from_value(value)?)
}

pub trait TaskNode: Send + Sync {
    fn execute(
        &self,
        inputs: Inputs,
        ctx: &mut crate::ExecutionContext,
    ) -> Result<crate::NodeResult, NodeExecutionError>;
}

#[derive(Clone, Debug, Default)]
pub struct NodeMetadata {
    pub ports: NodePorts,
    pub output_derivations: Vec<OutputDerivation>,
    pub context_references: Vec<ContextReference>,
    pub stdin: Option<crate::StdinRequirement>,
}

impl NodeMetadata {
    pub fn new(ports: NodePorts) -> Self {
        Self {
            ports,
            ..Self::default()
        }
    }
}

impl From<NodePorts> for NodeMetadata {
    fn from(ports: NodePorts) -> Self {
        Self::new(ports)
    }
}

pub struct PreparedNode {
    pub metadata: NodeMetadata,
    pub execution: NodeExecution,
}

pub enum NodeExecution {
    Task(Box<dyn TaskNode>),
    Event(Box<dyn crate::EventNode>),
    Stream(Box<dyn crate::StreamNode>),
}

impl NodeExecution {
    pub fn as_task_node(&self) -> Option<&dyn TaskNode> {
        match self {
            Self::Task(task) => Some(task.as_ref()),
            Self::Event(_) | Self::Stream(_) => None,
        }
    }

    pub fn into_task_node(self) -> Option<Box<dyn TaskNode>> {
        match self {
            Self::Task(task) => Some(task),
            Self::Event(_) | Self::Stream(_) => None,
        }
    }
}

impl PreparedNode {
    pub fn new(task: impl TaskNode + 'static, metadata: impl Into<NodeMetadata>) -> Self {
        Self {
            metadata: metadata.into(),
            execution: NodeExecution::Task(Box::new(task)),
        }
    }

    pub fn event(
        state: impl crate::EventNode + 'static,
        metadata: impl Into<NodeMetadata>,
    ) -> Self {
        Self {
            metadata: metadata.into(),
            execution: NodeExecution::Event(Box::new(state)),
        }
    }

    pub fn stream(
        producer: impl crate::StreamNode + 'static,
        metadata: impl Into<NodeMetadata>,
    ) -> Self {
        Self {
            metadata: metadata.into(),
            execution: NodeExecution::Stream(Box::new(producer)),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum NodeFactory {
    Plain(fn(Value) -> Result<PreparedNode, NodeBuildError>),
    Subgraph(
        fn(&str, Value, Value, crate::PreparedSubgraph) -> Result<PreparedNode, NodeBuildError>,
    ),
}

#[derive(Clone, Copy, Debug)]
pub struct NodeRegistration {
    pub kind: &'static str,
    pub factory: NodeFactory,
}

impl NodeRegistration {
    pub fn instantiate(&self, config: Value) -> Result<PreparedNode, NodeBuildError> {
        match self.factory {
            NodeFactory::Plain(factory) => factory(config),
            NodeFactory::Subgraph(_) => Err(NodeBuildError::InvalidSubgraph {
                message: format!("node `{}` requires a prepared body", self.kind),
            }),
        }
    }

    pub fn instantiate_subgraph(
        &self,
        id: &str,
        config: Value,
        options: Value,
        body: crate::PreparedSubgraph,
    ) -> Result<PreparedNode, NodeBuildError> {
        match self.factory {
            NodeFactory::Subgraph(factory) => factory(id, config, options, body),
            NodeFactory::Plain(_) => Err(NodeBuildError::InvalidSubgraph {
                message: format!("node `{}` does not accept a prepared body", self.kind),
            }),
        }
    }
}

inventory::collect!(NodeRegistration);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    static INTEGER_PORTS: &[PortSpec] = &[PortSpec::new("count", ValueType::Int64, true)];

    fn list(inner: ValueType) -> ValueType {
        ValueType::List(Box::new(inner))
    }

    fn map(inner: ValueType) -> ValueType {
        ValueType::Map(Box::new(inner))
    }

    #[test]
    fn classifies_static_checked_and_incompatible_connections() {
        use TypeCompatibility::{Checked, Incompatible, Static};
        use ValueType::{Any, Array, Float64, Int64, Number, Object, String};

        for (source, target, expected) in [
            (Int64, Number, Static),
            (Float64, Number, Static),
            (list(Int64), Array, Static),
            (map(String), Object, Static),
            (Array, list(Any), Static),
            (Object, map(Any), Static),
            (Any, Int64, Checked),
            (Number, Float64, Checked),
            (Array, list(Int64), Checked),
            (Object, map(String), Checked),
            (list(Number), list(Int64), Checked),
            (map(Any), map(String), Checked),
            (String, Int64, Incompatible),
            (Int64, Float64, Incompatible),
            (list(String), list(Int64), Incompatible),
            (map(String), map(Int64), Incompatible),
            (list(Int64), map(Int64), Incompatible),
        ] {
            assert_eq!(source.compatibility_with(&target), expected);
            assert_eq!(source.is_assignable_to(&target), expected == Static);
        }

        assert!(list(String).validate_value(&json!([])).is_ok());
        assert!(list(Int64).validate_value(&json!([])).is_ok());
        assert_eq!(list(String).compatibility_with(&list(Int64)), Incompatible);
        assert_eq!(INTEGER_PORTS[0].value_type, Int64);
    }

    #[test]
    fn checks_numeric_representations_without_coercion() {
        use ValueType::{Float64, Int64, Number};

        assert!(Int64.validate_value(&json!(i64::MIN)).is_ok());
        assert!(Int64.validate_value(&json!(i64::MAX)).is_ok());
        assert!(Int64.validate_value(&json!(1.0)).is_err());
        assert_eq!(
            Int64.validate_value(&json!(u64::MAX)).unwrap_err().actual,
            "unsigned integer"
        );
        assert!(Float64.validate_value(&json!(1.5)).is_ok());
        assert!(Float64.validate_value(&json!(1)).is_err());
        assert!(Number.validate_value(&json!(1)).is_ok());
        assert!(Number.validate_value(&json!(1.5)).is_ok());
        assert!(Number.validate_value(&json!(u64::MAX)).is_ok());
        for (value, actual) in [
            (json!(null), "null"),
            (json!([]), "array"),
            (json!({}), "object"),
        ] {
            assert_eq!(Int64.validate_value(&value).unwrap_err().actual, actual);
        }
    }

    #[test]
    fn reports_nested_paths_and_preserves_null() {
        let expected = list(map(ValueType::Int64));
        for (value, path, actual) in [
            (json!([{"a/b": 1}, {"a~b": "wrong"}]), "/1/a~0b", "string"),
            (
                json!([{"a/b": false, "z": null}, {"later": "wrong"}]),
                "/0/a~1b",
                "boolean",
            ),
            (json!([{"": false}]), "/0/", "boolean"),
        ] {
            let error = expected.validate_value(&value).unwrap_err();
            assert_eq!(error.path, path);
            assert_eq!(error.expected, ValueType::Int64);
            assert_eq!(error.actual, actual);
            assert_eq!(
                expected
                    .validate_shared(&crate::ValueRef::from(value))
                    .unwrap_err(),
                error
            );
        }
        assert_eq!(expected.validate_value(&json!(null)).unwrap_err().path, "");
        assert!(
            map(ValueType::Null)
                .validate_value(&json!({"value": null}))
                .is_ok()
        );
        assert!(ValueType::Any.validate_value(&json!(null)).is_ok());
        assert_eq!(format!("{expected}"), "list<map<int64>>");
        assert_eq!(
            map(ValueType::Int64)
                .validate_value(&json!({"a/b": false}))
                .unwrap_err()
                .path,
            "/a~1b"
        );
    }

    #[test]
    fn limits_descriptor_depth() {
        let mut valid = ValueType::Int64;
        for _ in 1..ValueType::MAX_DEPTH {
            valid = list(valid);
        }
        assert!(valid.check_depth().is_ok());
        let error = list(valid).check_depth().unwrap_err();
        assert_eq!(error.depth, ValueType::MAX_DEPTH + 1);
    }

    #[test]
    fn parses_shared_type_descriptors_without_relaxing_code_inputs() {
        assert_eq!(
            ValueType::parse_descriptor(&json!("any")).unwrap(),
            ValueType::Any
        );
        assert_eq!(
            ValueType::parse_descriptor(&json!({"list": {"map": "number"}})).unwrap(),
            list(map(ValueType::Number))
        );
        assert!(!ValueType::Any.is_concrete());
        assert!(!list(ValueType::Any).is_concrete());
        assert!(list(map(ValueType::Int64)).is_concrete());
        assert!(ValueType::parse_descriptor(&json!({"map": "unknown"})).is_err());
    }

    #[test]
    fn infers_json_literals_without_exceeding_descriptor_depth() {
        use ValueType::{Array, Boolean, Float64, Int64, Map, Null, Number, Object, String};

        for (value, expected) in [
            (json!(null), Null),
            (json!(false), Boolean),
            (json!(i64::MIN), Int64),
            (json!(i64::MAX), Int64),
            (json!(u64::MAX), Number),
            (json!(1.0), Float64),
            (json!("text"), String),
            (json!([]), Array),
            (json!({}), Object),
            (json!([1, "text"]), Array),
            (json!({"a": 1, "b": false}), Object),
            (json!([true, false]), list(Boolean)),
            (json!({"a": 1, "b": 2}), Map(Box::new(Int64))),
            (json!([{"count": 1}, {"count": 2}]), list(map(Int64))),
        ] {
            assert_eq!(ValueType::infer_json(&value), expected, "{value}");
        }

        let mut value = json!(1);
        for _ in 0..ValueType::MAX_DEPTH {
            value = json!([value]);
        }
        let mut inferred = ValueType::infer_json(&value);
        inferred.check_depth().unwrap();
        for _ in 1..ValueType::MAX_DEPTH {
            let ValueType::List(inner) = inferred else {
                panic!("expected a refined list before the depth boundary");
            };
            inferred = *inner;
        }
        assert_eq!(inferred, Array);
    }

    #[test]
    fn validates_output_derivations_against_instance_ports() {
        let ports = NodePorts {
            inputs: vec![PortSpec::new("input", ValueType::Any, true)],
            outputs: vec![PortSpec::new("value", ValueType::String, true)],
        };
        ports
            .validate_derivations(
                "fixture",
                &[OutputDerivation::forward_input("value", "input")],
            )
            .unwrap();
        ports
            .validate_derivations(
                "fixture",
                &[OutputDerivation::literal("value", json!("ok"))],
            )
            .unwrap();

        for (derivations, message) in [
            (
                vec![OutputDerivation::literal("missing", json!("x"))],
                "unknown output `missing`",
            ),
            (
                vec![OutputDerivation::forward_input("value", "missing")],
                "unknown input `missing`",
            ),
            (
                vec![OutputDerivation::literal("value", json!(42))],
                "output `value` literal conflicts",
            ),
            (
                vec![
                    OutputDerivation::literal("value", json!("a")),
                    OutputDerivation::literal("value", json!("b")),
                ],
                "more than one derivation",
            ),
        ] {
            let error = ports
                .validate_derivations("fixture", &derivations)
                .unwrap_err();
            assert!(error.to_string().contains("node `fixture`"), "{error}");
            assert!(error.to_string().contains(message), "{error}");
        }
    }
}
