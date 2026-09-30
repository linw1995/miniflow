use serde::de::DeserializeOwned;
use serde_json::Value;
use snafu::{ResultExt, Snafu};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

pub type Inputs = BTreeMap<String, Value>;
pub type Outputs = BTreeMap<String, Value>;

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
    pub fn descriptor(&self) -> Value {
        match self {
            Self::Any => Value::from("any"),
            Self::Null => Value::from("null"),
            Self::Boolean => Value::from("bool"),
            Self::Number => Value::from("number"),
            Self::Int64 => Value::from("int"),
            Self::Float64 => Value::from("double"),
            Self::String => Value::from("string"),
            Self::Array => Value::from("array"),
            Self::Object => Value::from("object"),
            Self::List(inner) => serde_json::json!({"list": inner.descriptor()}),
            Self::Map(inner) => serde_json::json!({"map": inner.descriptor()}),
        }
    }

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
        fn infer_at(value: &Value, depth: usize) -> ValueType {
            match value {
                Value::Null => ValueType::Null,
                Value::Bool(_) => ValueType::Boolean,
                Value::Number(number) if number.is_f64() => ValueType::Float64,
                Value::Number(number) if number.as_i64().is_some() => ValueType::Int64,
                Value::Number(_) => ValueType::Number,
                Value::String(_) => ValueType::String,
                Value::Array(items) if depth < ValueType::MAX_DEPTH && !items.is_empty() => {
                    let element = infer_at(&items[0], depth + 1);
                    if items[1..]
                        .iter()
                        .all(|item| infer_at(item, depth + 1) == element)
                    {
                        ValueType::List(Box::new(element))
                    } else {
                        ValueType::Array
                    }
                }
                Value::Array(_) => ValueType::Array,
                Value::Object(entries) if depth < ValueType::MAX_DEPTH && !entries.is_empty() => {
                    let first = entries
                        .values()
                        .next()
                        .expect("nonempty map has a first value");
                    let element = infer_at(first, depth + 1);
                    if entries
                        .values()
                        .skip(1)
                        .all(|item| infer_at(item, depth + 1) == element)
                    {
                        ValueType::Map(Box::new(element))
                    } else {
                        ValueType::Object
                    }
                }
                Value::Object(_) => ValueType::Object,
            }
        }

        infer_at(value, 1)
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
        self.validate_at(value, &mut String::new())
    }

    fn validate_at(&self, value: &Value, path: &mut String) -> Result<(), TypeMismatch> {
        let valid = match (self, value) {
            (Self::Any, _) => true,
            (Self::Null, Value::Null)
            | (Self::Boolean, Value::Bool(_))
            | (Self::Number | Self::Int64 | Self::Float64, Value::Number(_))
            | (Self::String, Value::String(_))
            | (Self::Array, Value::Array(_))
            | (Self::Object, Value::Object(_)) => match self {
                Self::Int64 => value
                    .as_number()
                    .is_some_and(|number| !number.is_f64() && number.as_i64().is_some()),
                Self::Float64 => value.as_number().is_some_and(|number| {
                    number.is_f64() && number.as_f64().is_some_and(f64::is_finite)
                }),
                _ => true,
            },
            (Self::List(inner), Value::Array(items)) => {
                for (index, item) in items.iter().enumerate() {
                    let previous = path.len();
                    path.push('/');
                    use fmt::Write as _;
                    write!(path, "{index}").expect("writing to a String cannot fail");
                    let result = inner.validate_at(item, path);
                    path.truncate(previous);
                    result?;
                }
                true
            }
            (Self::Map(inner), Value::Object(entries)) => {
                for (key, item) in entries {
                    let previous = path.len();
                    path.push('/');
                    for character in key.chars() {
                        match character {
                            '~' => path.push_str("~0"),
                            '/' => path.push_str("~1"),
                            _ => path.push(character),
                        }
                    }
                    let result = inner.validate_at(item, path);
                    path.truncate(previous);
                    result?;
                }
                true
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(TypeMismatch {
                path: path.clone(),
                expected: self.clone(),
                actual: actual_type(value),
            })
        }
    }
}

fn actual_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_f64() => "float64",
        Value::Number(number) if number.as_i64().is_some() => "int64",
        Value::Number(_) => "unsigned integer",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
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
    Literal { output: String, value: Value },
    ForwardInput { output: String, input: String },
}

impl OutputDerivation {
    pub fn literal(output: impl Into<String>, value: Value) -> Self {
        Self::Literal {
            output: output.into(),
            value,
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
            Self::Literal { output, .. } | Self::ForwardInput { output, .. } => output,
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
                OutputDerivation::Literal { value, .. } => port
                    .value_type
                    .validate_value(value)
                    .map_err(|source| OutputDerivationError::LiteralTypeMismatch {
                        node_id: node_id.to_owned(),
                        output: output.to_owned(),
                        source,
                    })?,
                OutputDerivation::ForwardInput { input, .. }
                    if !self.inputs.iter().any(|port| port.name == *input) =>
                {
                    return Err(OutputDerivationError::UnknownInput {
                        node_id: node_id.to_owned(),
                        output: output.to_owned(),
                        input: input.clone(),
                    });
                }
                OutputDerivation::ForwardInput { .. } => {}
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
    #[snafu(display("invalid node configuration: {source}"))]
    InvalidConfiguration { source: serde_json::Error },
    #[snafu(display("node factory failed: {source}"))]
    FactoryFailed {
        source: Box<dyn Error + Send + Sync + 'static>,
    },
}

#[derive(Debug, Snafu)]
pub enum NodeExecutionError {
    #[snafu(display("node execution failed: {message}"))]
    ExecutionFailed { message: String },
    #[snafu(display("node plugin failed: {source}"))]
    PluginFailed {
        source: Box<dyn Error + Send + Sync + 'static>,
    },
}

pub fn deserialize_config<T>(value: Value) -> Result<T, NodeBuildError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(value).context(InvalidConfigurationSnafu)
}

pub trait Node: Send + Sync {
    fn execute(&self, inputs: Inputs) -> Result<Outputs, NodeExecutionError>;

    fn subgraph_definition(
        &self,
        _definition: &crate::NodeDefinition,
    ) -> Result<Option<crate::SubgraphDefinition>, NodeBuildError> {
        Ok(None)
    }

    fn with_subgraph(
        self: Box<Self>,
        _id: &str,
        _options: Value,
        _body: crate::PreparedSubgraph,
    ) -> Result<Box<dyn Node>, NodeBuildError> {
        Err(NodeBuildError::InvalidSubgraph {
            message: "node does not support prepared subgraphs".into(),
        })
    }

    fn execute_with_context(
        &self,
        inputs: Inputs,
        _ctx: &crate::ExecutionContext,
    ) -> Result<crate::NodeResult, NodeExecutionError> {
        self.execute(inputs).map(Into::into)
    }

    fn execute_with_context_mut(
        &self,
        inputs: Inputs,
        ctx: &mut crate::ExecutionContext,
    ) -> Result<crate::NodeResult, NodeExecutionError> {
        self.execute_with_context(inputs, ctx)
    }

    fn ports(&self) -> Option<NodePorts> {
        None
    }

    fn output_derivations(&self) -> Vec<OutputDerivation> {
        Vec::new()
    }

    fn context_references(&self) -> Vec<ContextReference> {
        Vec::new()
    }
}

pub type NodeFactory = fn(Value) -> Result<Box<dyn Node>, NodeBuildError>;

#[derive(Clone, Copy, Debug)]
pub struct NodeRegistration {
    pub kind: &'static str,
    pub inputs: &'static [PortSpec],
    pub outputs: &'static [PortSpec],
    pub factory: NodeFactory,
}

impl NodeRegistration {
    pub fn instantiate(&self, config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
        (self.factory)(config)
    }

    pub fn effective_ports(&self, node: &dyn Node) -> NodePorts {
        node.ports().unwrap_or_else(|| NodePorts {
            inputs: self.inputs.to_vec(),
            outputs: self.outputs.to_vec(),
        })
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
        let error = expected
            .validate_value(&json!([{"a/b": 1}, {"a~b": "wrong"}]))
            .unwrap_err();
        assert_eq!(error.path, "/1/a~0b");
        assert_eq!(error.expected, ValueType::Int64);
        assert_eq!(error.actual, "string");
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
