use mf_runtime::{
    ContextReference, ContextValue, ExecutionContext, Inputs, Node, NodeBuildError,
    NodeExecutionError, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, ValueType,
    deserialize_config,
};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::{cmp::Ordering, collections::BTreeSet, fmt};

pub const KIND: &str = "builtin.if_else";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    branches: Vec<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Branch {
    id: String,
    condition: Condition,
}
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
struct Condition {
    source: Source,
    operator: Operator,
    #[serde(default, deserialize_with = "present_value")]
    value: Option<Value>,
}
fn present_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Debug)]
struct InvalidConfig(String);
impl fmt::Display for InvalidConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for InvalidConfig {}
fn invalid(message: impl Into<String>) -> NodeBuildError {
    NodeBuildError::FactoryFailed {
        source: Box::new(InvalidConfig(message.into())),
    }
}
fn execution_error(message: impl Into<String>) -> NodeExecutionError {
    NodeExecutionError::ExecutionFailed {
        message: message.into(),
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
    fn validate(&self) -> Result<(), String> {
        if self.source.output.is_empty() || !valid_pointer(&self.source.path) {
            return Err("source requires an output ID and a valid JSON Pointer".into());
        }
        match (self.operator, &self.value) {
            (Operator::Exists | Operator::NotExists, None) => Ok(()),
            (Operator::Eq | Operator::Ne, Some(value)) if !value.is_array() && !value.is_object() => Ok(()),
            (Operator::Gt | Operator::Gte | Operator::Lt | Operator::Lte, Some(Value::Number(_))) => Ok(()),
            _ => Err("operator requires a scalar comparison value, a numeric ordering value, or no value for existence checks".into()),
        }
    }
    fn evaluate(&self, source: Option<&mf_runtime::ValueRef>) -> Result<bool, String> {
        let value = source.and_then(|value| value.pointer(&self.source.path));
        match self.operator {
            Operator::Exists => return Ok(value.is_some()),
            Operator::NotExists => return Ok(value.is_none()),
            _ => {}
        }
        let value = value.ok_or("source output is skipped or field is missing")?;
        if value.is_array() || value.is_object() {
            return Err("comparison requires a scalar value".into());
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
                    return Err("ordering requires numeric operands".into());
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

struct IfElse {
    branches: Vec<Branch>,
}
impl Node for IfElse {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(execution_error(
            "if-else requires a workflow execution context",
        ))
    }
    fn ports(&self) -> Option<NodePorts> {
        Some(NodePorts {
            inputs: vec![],
            outputs: self
                .branches
                .iter()
                .map(|branch| branch.id.as_str())
                .chain(["else"])
                .map(|name| PortSpec::owned(name, ValueType::Boolean, false))
                .collect(),
        })
    }
    fn context_references(&self) -> Vec<ContextReference> {
        self.branches
            .iter()
            .map(|branch| ContextReference::new(&branch.condition.source.output, &branch.id))
            .collect()
    }
    fn execute_with_context(
        &self,
        _: Inputs,
        ctx: &ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let mut selected = "else";
        for branch in &self.branches {
            let condition = &branch.condition;
            let result = (|| {
                let source = match ctx
                    .output(&condition.source.output)
                    .map_err(|error| error.to_string())?
                {
                    ContextValue::Value(value) => Some(value),
                    ContextValue::Skipped => None,
                };
                condition.evaluate(source)
            })();
            let matched = result.map_err(|message| {
                execution_error(format!(
                    "branch `{}` source `{}` path `{}` operator {:?}: {message}",
                    branch.id, condition.source.output, condition.source.path, condition.operator,
                ))
            })?;
            if matched {
                selected = &branch.id;
                break;
            }
        }
        Ok(NodeResult {
            outputs: Outputs::from([(selected.to_owned(), Value::Bool(true).into())]),
            skipped: self
                .branches
                .iter()
                .map(|branch| branch.id.as_str())
                .chain(["else"])
                .filter(|name| *name != selected)
                .map(str::to_owned)
                .collect(),
            ..NodeResult::default()
        })
    }
}
fn factory(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    let config: Config = deserialize_config(config)?;
    if config.branches.is_empty() {
        return Err(invalid("branches must contain at least one condition"));
    }
    let mut ids = BTreeSet::new();
    let mut branches = Vec::with_capacity(config.branches.len());
    for (index, value) in config.branches.into_iter().enumerate() {
        let label = value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("index {index}"));
        let branch: Branch = deserialize_config(value)
            .map_err(|error| invalid(format!("branch `{label}`: {error}")))?;
        let mut bytes = branch.id.bytes();
        let valid = bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        if !valid || branch.id == "else" || !ids.insert(branch.id.clone()) {
            return Err(invalid(format!(
                "invalid, reserved, or duplicate branch ID `{}`",
                branch.id
            )));
        }
        branch
            .condition
            .validate()
            .map_err(|message| invalid(format!("branch `{}`: {message}", branch.id)))?;
        branches.push(branch);
    }
    Ok(Box::new(IfElse { branches }))
}
inventory::submit! { NodeRegistration { kind: KIND, inputs: &[], outputs: &[], factory } }

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn condition(operator: &str, value: Value) -> Value {
        json!({"source":{"output":"source.value","path":""},"operator":operator,"value":value})
    }
    fn config() -> Value {
        json!({"branches":[{"id":"accepted","condition":condition("eq", json!(true))}]})
    }
    #[test]
    fn requires_nonempty_unique_branches_and_valid_predicates() {
        assert!(factory(json!({})).is_err());
        assert!(factory(json!({"branches":[]})).is_err());
        for id in ["", "else", "1bad", "bad.name", "bad name"] {
            let mut value = config();
            value["branches"][0]["id"] = json!(id);
            assert!(factory(value).is_err(), "{id}");
        }
        let mut value = config();
        let duplicate = value["branches"][0].clone();
        value["branches"].as_array_mut().unwrap().push(duplicate);
        assert!(factory(value).is_err());
        for field in ["extra", "condition"] {
            let mut value = config();
            value["branches"][0][field] = json!(true);
            assert!(factory(value).is_err());
        }
        for predicate in [
            condition("unknown", json!(1)),
            condition("gt", json!("1")),
            condition("eq", json!([])),
            condition("ne", json!({})),
            condition("exists", json!(null)),
            json!({"source":{"output":"source.value","path":""},"operator":"eq"}),
            json!({"source":{"output":"","path":""},"operator":"exists"}),
        ] {
            let mut value = config();
            value["branches"][0]["condition"] = predicate;
            assert!(factory(value).is_err());
        }
        for path in ["amount", "/bad~", "/bad~2", "/bad~01~x"] {
            let mut value = config();
            value["branches"][0]["condition"]["source"]["path"] = json!(path);
            assert!(factory(value).is_err());
        }
        let node = factory(config()).unwrap();
        assert!(node.ports().unwrap().inputs.is_empty());
        assert_eq!(node.ports().unwrap().outputs.len(), 2);
        assert!(
            node.execute(Inputs::new())
                .unwrap_err()
                .to_string()
                .contains("context")
        );
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
                predicate.evaluate(Some(&input.clone().into())).unwrap(),
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
                    predicate.evaluate(Some(&input.clone().into())).unwrap(),
                    if operator == "exists" {
                        expected
                    } else {
                        !expected
                    }
                );
                assert_eq!(predicate.evaluate(None).unwrap(), operator == "not_exists");
            }
        }
        let eq: Condition = serde_json::from_value(condition("eq", json!(1))).unwrap();
        assert!(eq.evaluate(None).is_err());
        assert!(eq.evaluate(Some(&json!({}).into())).is_err());
        let gt: Condition = serde_json::from_value(condition("gt", json!(1))).unwrap();
        assert!(gt.evaluate(Some(&json!("2").into())).is_err());
    }
}
