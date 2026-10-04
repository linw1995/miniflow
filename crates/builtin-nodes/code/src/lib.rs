mod value;

use cel_core::types::{Expr, SpannedExpr};
use cel_core::{CelType, Env, MapActivation, Program, Value as CelValue};
use mf_runtime::{
    Inputs, NodeBuildError, NodeExecutionError, NodePorts, NodeRegistration, Outputs, PortSpec,
    TaskNode, ValueType, deserialize_config,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, error::Error, fmt};
use value::{Budget, MAX_JSON_BYTES, cel_to_json, json_to_cel};

pub const KIND: &str = "builtin.code";
const MAX_EXPRESSION_BYTES: usize = 8 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    language: String,
    inputs: BTreeMap<String, Value>,
    code: Value,
}

#[derive(Debug)]
struct InvalidConfig(String);

impl fmt::Display for InvalidConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for InvalidConfig {}

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

fn parse_type(value: &Value) -> Result<ValueType, String> {
    let value_type = ValueType::parse_descriptor(value)?;
    if !value_type.is_concrete() {
        return Err("type must be a concrete scalar, list, or map descriptor".into());
    }
    Ok(value_type)
}

fn cel_type(value_type: &ValueType) -> CelType {
    match value_type {
        ValueType::Null => CelType::Null,
        ValueType::Boolean => CelType::Bool,
        ValueType::Int64 => CelType::Int,
        ValueType::Float64 => CelType::Double,
        ValueType::String => CelType::String,
        ValueType::List(inner) => CelType::list(cel_type(inner)),
        ValueType::Map(inner) => CelType::map(CelType::String, cel_type(inner)),
        _ => unreachable!("configuration parsing accepts only concrete CEL types"),
    }
}

fn result_type(cel_type: &CelType) -> Result<ValueType, String> {
    match cel_type {
        CelType::Null => Ok(ValueType::Null),
        CelType::Bool => Ok(ValueType::Boolean),
        CelType::Int => Ok(ValueType::Int64),
        CelType::Double => Ok(ValueType::Float64),
        CelType::String => Ok(ValueType::String),
        CelType::List(inner) => Ok(ValueType::List(Box::new(result_type(inner)?))),
        CelType::Map(key, value) if key.as_ref() == &CelType::String => {
            Ok(ValueType::Map(Box::new(result_type(value)?)))
        }
        _ => Err(format!("unsupported inferred CEL result type `{cel_type}`")),
    }
}

fn valid_identifier(name: &str) -> bool {
    let Ok(ast) = Env::with_standard_library().parse_only(name) else {
        return false;
    };
    matches!(&ast.expr().node, cel_core::types::Expr::Ident(parsed) if parsed == name)
}

fn uses_explicit_dyn(expression: &SpannedExpr) -> bool {
    match &expression.node {
        Expr::Call { expr, args } => {
            matches!(&expr.node, Expr::Ident(name) | Expr::RootIdent(name) if name == "dyn")
                || uses_explicit_dyn(expr)
                || args.iter().any(uses_explicit_dyn)
        }
        Expr::Unary { expr, .. } | Expr::Member { expr, .. } => uses_explicit_dyn(expr),
        Expr::Binary { left, right, .. } => uses_explicit_dyn(left) || uses_explicit_dyn(right),
        Expr::Ternary {
            cond,
            then_expr,
            else_expr,
        } => {
            uses_explicit_dyn(cond) || uses_explicit_dyn(then_expr) || uses_explicit_dyn(else_expr)
        }
        Expr::Index { expr, index, .. } => uses_explicit_dyn(expr) || uses_explicit_dyn(index),
        Expr::List(items) => items.iter().any(|item| uses_explicit_dyn(&item.expr)),
        Expr::Map(entries) => entries
            .iter()
            .any(|entry| uses_explicit_dyn(&entry.key) || uses_explicit_dyn(&entry.value)),
        Expr::Struct { type_name, fields } => {
            uses_explicit_dyn(type_name)
                || fields.iter().any(|field| uses_explicit_dyn(&field.value))
        }
        Expr::Comprehension(data) => [
            &data.iter_range,
            &data.accu_init,
            &data.loop_condition,
            &data.loop_step,
            &data.result,
        ]
        .into_iter()
        .any(|part| uses_explicit_dyn(part)),
        Expr::MemberTestOnly { expr, .. } => uses_explicit_dyn(expr),
        Expr::Bind { init, body, .. } => uses_explicit_dyn(init) || uses_explicit_dyn(body),
        _ => false,
    }
}

fn json_size(value: &(impl Serialize + ?Sized)) -> Result<usize, serde_json::Error> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_JSON_BYTES.saturating_sub(self.0) {
                self.0 = MAX_JSON_BYTES + 1;
                return Err(std::io::Error::other("JSON byte limit"));
            }
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    let result = serde_json::to_writer(&mut counter, value);
    if counter.0 <= MAX_JSON_BYTES {
        result?;
    }
    Ok(counter.0)
}

struct CodeNode {
    ports: NodePorts,
    programs: BTreeMap<String, Program>,
}

impl TaskNode for CodeNode {
    fn execute(
        &self,
        inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        let input_size = json_size(&inputs)
            .map_err(|error| execution_error(format!("could not measure inputs: {error}")))?;
        if input_size > MAX_JSON_BYTES {
            return Err(execution_error(format!(
                "inputs exceed the {MAX_JSON_BYTES}-byte JSON limit"
            )));
        }
        for name in inputs.keys() {
            if !self.ports.inputs.iter().any(|port| port.name == *name) {
                return Err(execution_error(format!("undeclared input `{name}`")));
            }
        }
        let mut budget = Budget::default();
        let mut activation = MapActivation::new();
        for port in &self.ports.inputs {
            let name = port.name.as_ref();
            let value = inputs
                .get(name)
                .ok_or_else(|| execution_error(format!("missing input `{name}`")))?;
            let converted = json_to_cel(value, &port.value_type, &mut budget, "", 1)
                .map_err(|error| execution_error(format!("input `{name}`: {error}")))?;
            activation.insert(name, converted);
        }
        let mut outputs = Outputs::new();
        let mut output_size = 2usize;
        for port in &self.ports.outputs {
            let name = port.name.as_ref();
            let result = self.programs[name].eval(&activation);
            if let CelValue::Error(error) = &result {
                return Err(execution_error(format!("output `{name}`: {error}")));
            }
            let converted = cel_to_json(&result, &port.value_type, &mut budget, "", 1)
                .map_err(|error| execution_error(format!("output `{name}`: {error}")))?;
            let encoded_name = json_size(name)
                .map_err(|error| execution_error(format!("output `{name}`: {error}")))?;
            let encoded_value = json_size(&converted)
                .map_err(|error| execution_error(format!("output `{name}`: {error}")))?;
            output_size += encoded_name + encoded_value + 1 + usize::from(!outputs.is_empty());
            if output_size > MAX_JSON_BYTES {
                return Err(execution_error(format!(
                    "output `{name}`: outputs exceed the {MAX_JSON_BYTES}-byte JSON limit"
                )));
            }
            outputs.insert(name.to_owned(), converted);
        }
        Ok(outputs.into())
    }
}
impl CodeNode {
    fn ports(&self) -> NodePorts {
        self.ports.clone()
    }
}

fn factory(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let config: Config = deserialize_config(config)?;
    if config.language != "cel" {
        return Err(invalid(format!(
            "unsupported language `{}`",
            config.language
        )));
    }
    let mut env = Env::with_standard_library();
    let mut inputs = Vec::with_capacity(config.inputs.len());
    for (name, descriptor) in &config.inputs {
        if !valid_identifier(name) {
            return Err(invalid(format!("invalid input name `{name}`")));
        }
        let value_type =
            parse_type(descriptor).map_err(|error| invalid(format!("input `{name}`: {error}")))?;
        env = env.with_variable(name, cel_type(&value_type));
        inputs.push(PortSpec::owned(name, value_type, true));
    }
    let code = config
        .code
        .as_object()
        .filter(|code| !code.is_empty())
        .ok_or_else(|| invalid("code must map output names to CEL expressions"))?;
    let mut outputs = Vec::with_capacity(code.len());
    let mut programs = BTreeMap::new();
    for (name, expression) in code {
        if !valid_identifier(name) {
            return Err(invalid(format!("invalid output name `{name}`")));
        }
        let expression = expression
            .as_str()
            .filter(|expression| !expression.trim().is_empty())
            .ok_or_else(|| invalid(format!("output `{name}` requires a nonblank expression")))?;
        if expression.len() > MAX_EXPRESSION_BYTES {
            return Err(invalid(format!(
                "output `{name}` exceeds the {MAX_EXPRESSION_BYTES}-byte expression limit"
            )));
        }
        let ast = env
            .compile(expression)
            .map_err(|error| invalid(format!("output `{name}`: {error}")))?;
        // CEL macros can contain internal dynamic types even when the user expression is concrete.
        if uses_explicit_dyn(ast.expr()) {
            return Err(invalid(format!(
                "output `{name}`: explicit dyn(...) is unsupported"
            )));
        }
        let inferred = ast
            .result_type()
            .ok_or_else(|| invalid(format!("output `{name}` has no inferred type")))?;
        let value_type =
            result_type(inferred).map_err(|error| invalid(format!("output `{name}`: {error}")))?;
        value_type
            .check_depth()
            .map_err(|error| invalid(format!("output `{name}`: {error}")))?;
        let program = env
            .program(&ast)
            .map_err(|error| invalid(format!("output `{name}`: {error}")))?;
        outputs.push(PortSpec::owned(name, value_type, true));
        programs.insert(name.clone(), program);
    }
    let node = CodeNode {
        ports: NodePorts { inputs, outputs },
        programs,
    };
    let metadata = mf_runtime::NodeMetadata::new(node.ports());
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(factory) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cel_core::{MapActivation, Value as CelValue};
    use serde_json::json;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn pinned_cel_api_checks_and_evaluates_json_values() {
        assert_send_sync::<Program>();
        let env = Env::with_standard_library().with_variable("amount", CelType::Int);
        let ast = env.compile("amount * 2").unwrap();
        assert_eq!(ast.result_type(), Some(&CelType::Int));
        let mut activation = MapActivation::new();
        activation.insert("amount", json!(21).as_i64().unwrap());
        let result = env.program(&ast).unwrap().eval(&activation);
        assert!(matches!(result, CelValue::Int(42)));
    }

    #[test]
    fn registers_one_code_kind_and_infers_instance_ports() {
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let registration = registry.get(KIND).unwrap();
        for (config, expected_input, expected_output) in [
            (
                json!({"language":"cel","inputs":{"amount":"int"},"code":{"doubled":"amount * 2"}}),
                ValueType::Int64,
                ValueType::Int64,
            ),
            (
                json!({"language":"cel","inputs":{"items":{"list":"int"}},"code":{"doubled":"items.map(x, x * 2)"}}),
                ValueType::List(Box::new(ValueType::Int64)),
                ValueType::List(Box::new(ValueType::Int64)),
            ),
            (
                json!({"language":"cel","inputs":{"values":{"map":"int"}},"code":{"count":"values['a']"}}),
                ValueType::Map(Box::new(ValueType::Int64)),
                ValueType::Int64,
            ),
        ] {
            let node = registration.instantiate(config).unwrap();
            let ports = &node.metadata.ports;
            assert_eq!(ports.inputs[0].value_type, expected_input);
            assert_eq!(ports.outputs[0].value_type, expected_output);
        }
    }

    #[test]
    fn rejects_invalid_language_and_config_shapes() {
        for config in [
            json!({"language":"rust","inputs":{},"code":"fn main() {}"}),
            json!({"language":"cel","inputs":{},"code":{}}),
            json!({"language":"cel","inputs":{"amount":"any"},"code":{"result":"1"}}),
            json!({"language":"cel","inputs":{"items":{"list":{"map":"any"}}},"code":{"result":"1"}}),
            json!({"language":"cel","inputs":{"a.b":"int"},"code":{"result":"1"}}),
            json!({"language":"cel","inputs":{},"code":{"bad.name":"1"}}),
            json!({"language":"cel","inputs":{},"code":{"result":"  "}}),
            json!({"language":"cel","inputs":{},"code":{"result":"1"},"extra":true}),
        ] {
            assert!(factory(config.clone()).is_err(), "{config}");
        }
        let mut too_deep = json!("int");
        for _ in 0..ValueType::MAX_DEPTH {
            too_deep = json!({"list": too_deep});
        }
        assert!(
            factory(json!({
                "language": "cel",
                "inputs": {"items": too_deep},
                "code": {"result": "1"}
            }))
            .is_err()
        );
    }

    #[test]
    fn reports_invalid_descriptors_and_names() {
        for (descriptor, expected) in [
            (json!({"set": "int"}), "unsupported type constructor"),
            (json!(["int"]), "type must be"),
            (json!({"list": "int", "map": "int"}), "type must be"),
        ] {
            let error = factory(json!({
                "language": "cel",
                "inputs": {"item": descriptor},
                "code": {"result": "1"}
            }))
            .err()
            .unwrap()
            .to_string();
            assert!(error.contains("input `item`") && error.contains(expected));
        }
        let error = factory(json!({
            "language": "cel",
            "inputs": {"two words": "int"},
            "code": {"result": "1"}
        }))
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains("invalid input name"));
    }

    #[test]
    fn maps_each_supported_input_and_output_type() {
        for (input_type, expression, expected) in [
            ("null", "value", ValueType::Null),
            ("bool", "value", ValueType::Boolean),
            ("int", "value", ValueType::Int64),
            ("double", "value", ValueType::Float64),
            ("string", "value", ValueType::String),
        ] {
            let node = factory(json!({
                "language": "cel",
                "inputs": {"value": input_type},
                "code": {"result": expression}
            }))
            .unwrap();
            assert_eq!(node.metadata.ports.outputs[0].value_type, expected);
        }
        let node = factory(json!({
            "language": "cel",
            "inputs": {},
            "code": {"result": "{'count': 1}"}
        }))
        .unwrap();
        assert_eq!(
            node.metadata.ports.outputs[0].value_type,
            ValueType::Map(Box::new(ValueType::Int64))
        );
    }

    #[test]
    fn reports_non_json_result_types() {
        for expression in ["b'bytes'", "{1: 2}", "dyn(1)"] {
            let error = factory(json!({
                "language": "cel",
                "inputs": {},
                "code": {"result": expression}
            }))
            .err()
            .unwrap()
            .to_string();
            assert!(error.contains("output `result`"), "{expression}: {error}");
        }
    }

    #[test]
    fn evaluates_scalar_and_collection_outputs_from_one_activation() {
        let node = factory(json!({
            "language": "cel",
            "inputs": {
                "amount": "int",
                "items": {"list": "int"},
                "values": {"map": "int"},
                "nothing": "null"
            },
            "code": {
                "doubled": "amount * 2",
                "list": "items.map(x, x * 2)",
                "map": "values",
                "empty": "nothing"
            }
        }))
        .unwrap();
        let outputs = node
            .execution
            .as_task_node()
            .expect("expected task execution")
            .execute(
                Inputs::from([
                    ("amount".into(), json!(21).into()),
                    ("items".into(), json!([1, 2]).into()),
                    ("values".into(), json!({"a": 3}).into()),
                    ("nothing".into(), Value::Null.into()),
                ]),
                &mut mf_runtime::ExecutionContext::default(),
            )
            .unwrap()
            .outputs;
        assert_eq!(outputs["doubled"], json!(42));
        assert_eq!(outputs["list"], json!([2, 4]));
        assert_eq!(outputs["map"], json!({"a": 3}));
        assert_eq!(outputs["empty"], Value::Null);
    }

    #[test]
    fn checks_inputs_even_when_called_without_a_workflow() {
        let node = factory(json!({
            "language": "cel",
            "inputs": {"items": {"list": {"map": "int"}}},
            "code": {"result": "items"}
        }))
        .unwrap();
        for (inputs, expected) in [
            (Inputs::new(), "missing input `items`"),
            (
                Inputs::from([("extra".into(), json!(1).into())]),
                "undeclared input `extra`",
            ),
            (
                Inputs::from([("items".into(), json!(null).into())]),
                "expected list",
            ),
            (
                Inputs::from([("items".into(), json!([{"a/b": 1}, {"a~b": false}]).into())]),
                "path `/1/a~0b`",
            ),
            (
                Inputs::from([("items".into(), json!([{"a": "wrong"}]).into())]),
                "path `/0/a`",
            ),
        ] {
            let error = node
                .execution
                .as_task_node()
                .expect("expected task execution")
                .execute(inputs, &mut mf_runtime::ExecutionContext::default())
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
        let int_node = factory(json!({
            "language": "cel",
            "inputs": {"amount": "int"},
            "code": {"result": "amount"}
        }))
        .unwrap();
        let error = int_node
            .execution
            .as_task_node()
            .expect("expected task execution")
            .execute(
                Inputs::from([("amount".into(), json!(u64::MAX).into())]),
                &mut mf_runtime::ExecutionContext::default(),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("input `amount`") && error.contains("expected int64"));
    }

    #[test]
    fn reports_evaluation_errors_without_returning_earlier_outputs() {
        let node = factory(json!({
            "language": "cel",
            "inputs": {"divisor": "int"},
            "code": {"first": "1", "second": "1 / divisor"}
        }))
        .unwrap();
        let error = node
            .execution
            .as_task_node()
            .expect("expected task execution")
            .execute(
                Inputs::from([("divisor".into(), json!(0).into())]),
                &mut mf_runtime::ExecutionContext::default(),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("output `second`"), "{error}");
    }

    #[test]
    fn enforces_expression_payload_and_collection_limits() {
        let expression = format!("1{}", " ".repeat(MAX_EXPRESSION_BYTES));
        let error = factory(json!({
            "language": "cel",
            "inputs": {},
            "code": {"result": expression}
        }))
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains("expression limit"));

        let node = factory(json!({
            "language": "cel",
            "inputs": {"payload": "string"},
            "code": {"result": "payload + payload"}
        }))
        .unwrap();
        let error = node
            .execution
            .as_task_node()
            .expect("expected task execution")
            .execute(
                Inputs::from([(
                    "payload".into(),
                    json!("x".repeat(MAX_JSON_BYTES + 1)).into(),
                )]),
                &mut mf_runtime::ExecutionContext::default(),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("inputs exceed"), "{error}");
        let error = node
            .execution
            .as_task_node()
            .expect("expected task execution")
            .execute(
                Inputs::from([("payload".into(), json!("x".repeat(600_000)).into())]),
                &mut mf_runtime::ExecutionContext::default(),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("output `result`: outputs exceed"), "{error}");

        let node = factory(json!({
            "language": "cel",
            "inputs": {"items": {"list": "int"}},
            "code": {"first": "items", "second": "items"}
        }))
        .unwrap();
        let error = node
            .execution
            .as_task_node()
            .expect("expected task execution")
            .execute(
                Inputs::from([("items".into(), json!(vec![1; 4_000]).into())]),
                &mut mf_runtime::ExecutionContext::default(),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("output `second`"), "{error}");
        assert!(error.contains("collection entry limit"), "{error}");
    }

    #[test]
    fn accepts_exact_json_byte_limits_and_rejects_one_extra_byte() {
        let payload = |bytes: usize| {
            let unit = "\0\\\"\n\u{1f642}";
            let encoded = serde_json::to_vec(unit).unwrap().len() - 2;
            format!(
                "{}{}",
                unit.repeat(bytes / encoded),
                "x".repeat(bytes % encoded)
            )
        };
        for (input, output, input_limited) in [("payload", "out", true), ("x", "result", false)] {
            let node = factory(json!({
                "language": "cel",
                "inputs": BTreeMap::from([(input, "string")]),
                "code": BTreeMap::from([(output, input)])
            }))
            .unwrap();
            let bounded_name = if input_limited { input } else { output };
            let overhead = serde_json::to_vec(&BTreeMap::from([(bounded_name, "")]))
                .unwrap()
                .len();
            let exact = payload(MAX_JSON_BYTES - overhead);
            let inputs = Inputs::from([(input.into(), exact.clone().into())]);
            let outputs = node
                .execution
                .as_task_node()
                .expect("expected task execution")
                .execute(inputs, &mut mf_runtime::ExecutionContext::default())
                .unwrap()
                .outputs;
            assert_eq!(outputs[output].as_str(), Some(exact.as_str()));
            let error = node
                .execution
                .as_task_node()
                .expect("expected task execution")
                .execute(
                    Inputs::from([(input.into(), format!("{exact}x").into())]),
                    &mut mf_runtime::ExecutionContext::default(),
                )
                .unwrap_err()
                .to_string();
            let expected = if input_limited {
                "inputs exceed".to_owned()
            } else {
                format!("output `{output}`: outputs exceed")
            };
            assert!(error.contains(&expected), "{error}");
        }
    }

    #[test]
    fn infers_boolean_and_nested_collection_results() {
        let node = factory(json!({
            "language": "cel",
            "inputs": {"amount": "int"},
            "code": {
                "boolean": "amount > 0",
                "nested": "[{'count': amount}]"
            }
        }))
        .unwrap();
        let ports = node.metadata.ports;
        let types: BTreeMap<_, _> = ports
            .outputs
            .iter()
            .map(|port| (port.name.as_ref(), &port.value_type))
            .collect();
        assert_eq!(types["boolean"], &ValueType::Boolean);
        assert_eq!(
            types["nested"],
            &ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Int64))))
        );
    }

    #[test]
    fn rejects_static_errors_and_explicit_dynamic_calls() {
        for expression in [
            "missing + 1",
            "amount + 'x'",
            "amount *",
            "dyn(amount)",
            "int(dyn(amount))",
            "[amount, 'x']",
            "b'bytes'",
        ] {
            let error = factory(json!({
                "language": "cel",
                "inputs": {"amount": "int"},
                "code": {"result": expression}
            }))
            .err()
            .unwrap()
            .to_string();
            assert!(error.contains("output `result`"), "{expression}: {error}");
        }
        assert!(
            factory(json!({
                "language": "cel",
                "inputs": {"items": {"list": "int"}},
                "code": {"result": "items.map(x, x * 2)"}
            }))
            .is_ok()
        );
    }

    #[test]
    fn documented_config_parses_in_a_workflow_definition() {
        mf_runtime::WorkflowDefinition::from_json(
            r#"{
                "version": "2026-09-26",
                "dependencies": {"code": {"package": "mfn-code", "version": "=0.1.0"}},
                "nodes": [{"id": "transform", "kind": "builtin.code", "config": {
                    "language": "cel",
                    "inputs": {"amount": "int"},
                    "code": {"doubled": "amount * 2"}
                }}],
                "outputs": [{"name": "result", "node": "transform", "port": "doubled"}]
            }"#,
        )
        .unwrap();
    }
}
