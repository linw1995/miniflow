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
    pub const MAX_DEPTH: usize = 16;

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

    fn execute_with_context(
        &self,
        inputs: Inputs,
        _ctx: &crate::ExecutionContext,
    ) -> Result<crate::NodeResult, NodeExecutionError> {
        self.execute(inputs).map(Into::into)
    }

    fn ports(&self) -> Option<NodePorts> {
        None
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
}
