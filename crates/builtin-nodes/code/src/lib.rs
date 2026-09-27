use cel_core::types::{Expr, SpannedExpr};
use cel_core::{CelType, Env, Program};
use mf_runtime::{
    Inputs, Node, NodeBuildError, NodeExecutionError, NodePorts, NodeRegistration, Outputs,
    PortSpec, ValueType, deserialize_config,
};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, error::Error, fmt};

pub const KIND: &str = "builtin.code";

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

fn parse_type(value: &Value, depth: usize) -> Result<ValueType, String> {
    if depth > ValueType::MAX_DEPTH {
        return Err(format!(
            "type nesting depth exceeds {}",
            ValueType::MAX_DEPTH
        ));
    }
    match value {
        Value::String(name) => match name.as_str() {
            "int" => Ok(ValueType::Int64),
            "double" => Ok(ValueType::Float64),
            "bool" => Ok(ValueType::Boolean),
            "string" => Ok(ValueType::String),
            "null" => Ok(ValueType::Null),
            _ => Err(format!("unsupported type `{name}`")),
        },
        Value::Object(fields) if fields.len() == 1 => {
            let (kind, inner) = fields.iter().next().unwrap();
            match kind.as_str() {
                "list" => Ok(ValueType::List(Box::new(parse_type(inner, depth + 1)?))),
                "map" => Ok(ValueType::Map(Box::new(parse_type(inner, depth + 1)?))),
                _ => Err(format!("unsupported type constructor `{kind}`")),
            }
        }
        _ => Err("type must be a concrete scalar, list, or map descriptor".into()),
    }
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

struct CodeNode {
    ports: NodePorts,
    programs: BTreeMap<String, Program>,
}

impl Node for CodeNode {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        let _ = &self.programs;
        Err(NodeExecutionError::ExecutionFailed {
            message: "CEL execution is not available".into(),
        })
    }

    fn ports(&self) -> Option<NodePorts> {
        Some(self.ports.clone())
    }
}

fn factory(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
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
        let value_type = parse_type(descriptor, 1)
            .map_err(|error| invalid(format!("input `{name}`: {error}")))?;
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
    Ok(Box::new(CodeNode {
        ports: NodePorts { inputs, outputs },
        programs,
    }))
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        inputs: &[],
        outputs: &[],
        factory,
    }
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
            let ports = registration.effective_ports(node.as_ref());
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
            assert_eq!(node.ports().unwrap().outputs[0].value_type, expected);
        }
        let node = factory(json!({
            "language": "cel",
            "inputs": {},
            "code": {"result": "{'count': 1}"}
        }))
        .unwrap();
        assert_eq!(
            node.ports().unwrap().outputs[0].value_type,
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
        let ports = node.ports().unwrap();
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
