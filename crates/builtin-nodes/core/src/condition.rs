use mf_runtime::{
    ContextReference, ContextValue, ExecutionContext, NodeBuildError, NodeExecutionError,
};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use snafu::{OptionExt, ResultExt, Snafu};
use std::cmp::Ordering;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    output: String,
    path: String,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operator {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    Exists,
    NotExists,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    source: Source,
    operator: Operator,
    #[serde(default, deserialize_with = "present_value")]
    value: Option<Value>,
}
fn present_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Debug, Snafu)]
#[snafu(display("{message}"))]
pub struct ConditionValidationError {
    message: &'static str,
}

impl From<ConditionValidationError> for NodeBuildError {
    fn from(source: ConditionValidationError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

#[derive(Debug, Snafu)]
#[snafu(display("source `{output}` path `{path}` operator {operator:?}: {source}"))]
pub struct ConditionExecutionError {
    output: String,
    path: String,
    operator: Operator,
    source: NodeExecutionError,
}

impl From<ConditionExecutionError> for NodeExecutionError {
    fn from(source: ConditionExecutionError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

#[derive(Debug, Snafu)]
enum ConditionValueError {
    #[snafu(display("source output is skipped or field is missing"))]
    MissingSource,
    #[snafu(display("comparison requires a scalar value"))]
    NonScalar,
    #[snafu(display("ordering requires numeric operands"))]
    NonNumeric,
}

impl From<ConditionValueError> for NodeExecutionError {
    fn from(source: ConditionValueError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

fn valid_pointer(path: &str) -> bool {
    if !path.is_empty() && !path.starts_with('/') {
        return false;
    }
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'~' && !matches!(bytes.next(), Some(b'0' | b'1')) {
            return false;
        }
    }
    true
}
impl Condition {
    pub fn validate(&self) -> Result<(), ConditionValidationError> {
        if self.source.output.is_empty() || !valid_pointer(&self.source.path) {
            return ConditionValidationSnafu {
                message: "source requires an output ID and a valid JSON Pointer",
            }
            .fail();
        }
        match (self.operator, &self.value) {
            (Operator::Exists | Operator::NotExists, None) => Ok(()),
            (Operator::Eq | Operator::Ne, Some(value)) if !value.is_array() && !value.is_object() => Ok(()),
            (Operator::Gt | Operator::Gte | Operator::Lt | Operator::Lte, Some(Value::Number(_))) => Ok(()),
            _ => ConditionValidationSnafu { message: "operator requires a scalar comparison value, a numeric ordering value, or no value for existence checks" }.fail(),
        }
    }
    fn evaluate_value(
        &self,
        source: Option<&mf_runtime::ValueRef>,
    ) -> Result<bool, ConditionValueError> {
        let value = source.and_then(|value| value.pointer(&self.source.path));
        match self.operator {
            Operator::Exists => return Ok(value.is_some()),
            Operator::NotExists => return Ok(value.is_none()),
            _ => {}
        }
        let value = value.context(MissingSourceSnafu)?;
        if value.is_array() || value.is_object() {
            return NonScalarSnafu.fail();
        }
        let literal = self
            .value
            .as_ref()
            .expect("comparison literals are validated during construction");
        match self.operator {
            Operator::Eq | Operator::Ne => {
                let equal = match (value.as_number(), literal.as_number()) {
                    (Some(left), Some(right)) => {
                        mf_runtime::compare_json_numbers(left, right) == Ordering::Equal
                    }
                    _ => value == literal,
                };
                Ok(if matches!(self.operator, Operator::Eq) {
                    equal
                } else {
                    !equal
                })
            }
            operator => {
                let (Some(left), Some(right)) = (value.as_number(), literal.as_number()) else {
                    return NonNumericSnafu.fail();
                };
                let order = mf_runtime::compare_json_numbers(left, right);
                Ok(match operator {
                    Operator::Gt => order == Ordering::Greater,
                    Operator::Gte => order != Ordering::Less,
                    Operator::Lt => order == Ordering::Less,
                    Operator::Lte => order != Ordering::Greater,
                    _ => unreachable!("equality and existence were handled above"),
                })
            }
        }
    }
}

impl Condition {
    pub fn reference(&self, label: &str) -> ContextReference {
        ContextReference::new(&self.source.output, label)
    }

    pub fn evaluate(&self, context: &ExecutionContext) -> Result<bool, ConditionExecutionError> {
        let result = (|| {
            let source = match context.output(&self.source.output)? {
                ContextValue::Value(value) => Some(value),
                ContextValue::Skipped => None,
            };
            self.evaluate_value(source)
                .map_err(NodeExecutionError::from)
        })();
        result.context(ConditionExecutionSnafu {
            output: &self.source.output,
            path: &self.source.path,
            operator: self.operator,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_failures_preserve_the_typed_value_error() {
        use std::error::Error;
        let condition: Condition = serde_json::from_value(serde_json::json!({
            "source": {"output": "source.value", "path": ""},
            "operator": "eq", "value": 1
        }))
        .unwrap();
        condition.validate().unwrap();
        let error = condition
            .evaluate_value(Some(&mf_runtime::ValueRef::from(serde_json::json!([]))))
            .unwrap_err();
        let error: NodeExecutionError = error.into();
        assert!(matches!(
            error
                .source()
                .unwrap()
                .downcast_ref::<ConditionValueError>(),
            Some(ConditionValueError::NonScalar)
        ));
    }

    use serde_json::json;

    fn condition(operator: &str, value: Value) -> Value {
        json!({"source":{"output":"source.value","path":""},"operator":operator,"value":value})
    }
    #[test]
    fn evaluates_typed_scalars_presence_and_json_pointers() {
        for (operator, input, literal, expected) in [
            ("eq", json!(1), json!(1.0), true),
            ("ne", json!("1"), json!(1), true),
            ("eq", json!("VIP"), json!("vip"), false),
            ("eq", json!(null), json!(null), true),
            ("gt", json!(2), json!(1), true),
            ("gte", json!(1), json!(1), true),
            ("lt", json!(1), json!(2), true),
            ("lte", json!(1), json!(1), true),
        ] {
            let predicate: Condition =
                serde_json::from_value(condition(operator, literal)).unwrap();
            predicate.validate().unwrap();
            assert_eq!(
                predicate
                    .evaluate_value(Some(&input.clone().into()))
                    .unwrap(),
                expected
            );
        }
        let input = json!({"a/b":null,"a~b":true,"items":[{"price":10}]});
        for (path, expected) in [
            ("/a~1b", true),
            ("/a~0b", true),
            ("/items/0/price", true),
            ("/missing", false),
            ("/items/9", false),
            ("/a~0b/no", false),
        ] {
            for operator in ["exists", "not_exists"] {
                let predicate: Condition = serde_json::from_value(
                    json!({"source":{"output":"a.value","path":path},"operator":operator}),
                )
                .unwrap();
                predicate.validate().unwrap();
                assert_eq!(
                    predicate
                        .evaluate_value(Some(&input.clone().into()))
                        .unwrap(),
                    if operator == "exists" {
                        expected
                    } else {
                        !expected
                    }
                );
                assert_eq!(
                    predicate.evaluate_value(None).unwrap(),
                    operator == "not_exists"
                );
            }
        }
        let eq: Condition = serde_json::from_value(condition("eq", json!(1))).unwrap();
        assert!(eq.evaluate_value(None).is_err());
        assert!(eq.evaluate_value(Some(&json!({}).into())).is_err());
        let gt: Condition = serde_json::from_value(condition("gt", json!(1))).unwrap();
        assert!(gt.evaluate_value(Some(&json!("2").into())).is_err());
    }
}
