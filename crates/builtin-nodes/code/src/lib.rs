mod value;

use cel_core::types::{Expr, SpannedExpr};
use cel_core::{CelType, Env, MapActivation, Program, Value as CelValue};
use mf_runtime::{
    Inputs, NodeBuildError, NodeExecutionError, NodePortContract, NodePorts, NodeRegistration,
    Outputs, PortSpec, TaskNode, ValueType, deserialize_config,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use snafu::{ResultExt, Snafu};
use std::{collections::BTreeMap, error::Error};
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

#[derive(Debug, Snafu)]
enum CodeBuildError {
    #[snafu(display("{message}"))]
    InvalidConfig { message: String },
    #[snafu(display("output `{output}`: {source}"))]
    Compile {
        output: String,
        source: cel_core::CompileError,
    },
    #[snafu(display("output `{output}`: {source}"))]
    Depth {
        output: String,
        source: mf_runtime::TypeDepthError,
    },
}

impl From<CodeBuildError> for NodeBuildError {
    fn from(source: CodeBuildError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

#[derive(Debug, Snafu)]
enum CodeExecutionError {
    #[snafu(display("could not measure inputs: {source}"))]
    MeasureInputs { source: serde_json::Error },
    #[snafu(display("output `{output}`: {source}"))]
    MeasureOutput {
        output: String,
        source: serde_json::Error,
    },
    #[snafu(display("output `{output}`: {source}"))]
    Evaluate {
        output: String,
        source: cel_core::EvalError,
    },
}

impl From<CodeExecutionError> for NodeExecutionError {
    fn from(source: CodeExecutionError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

fn invalid(message: impl Into<String>) -> NodeBuildError {
    InvalidConfigSnafu {
        message: message.into(),
    }
    .build()
    .into()
}

fn execution_error(message: impl Into<String>) -> NodeExecutionError {
    mf_runtime::NodeExecutionFailedSnafu {
        message: message.into(),
    }
    .build()
}

fn parse_type(value: &Value) -> Result<(ValueType, CelType), String> {
    let value_type = ValueType::parse_descriptor(value)?;
    let native_type = cel_type(&value_type)?;
    Ok((value_type, native_type))
}

fn cel_type(value_type: &ValueType) -> Result<CelType, String> {
    Ok(match value_type {
        ValueType::Null => CelType::Null,
        ValueType::Boolean => CelType::Bool,
        ValueType::Int64 => CelType::Int,
        ValueType::Float64 => CelType::Double,
        ValueType::String => CelType::String,
        ValueType::List(inner) => CelType::list(cel_type(inner)?),
        ValueType::Map(inner) => CelType::map(CelType::String, cel_type(inner)?),
        _ => return Err("type must be a concrete CEL scalar, list, or map descriptor".into()),
    })
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

struct CheckedOutput {
    program: Program,
    value_type: ValueType,
}

struct CodeNode {
    inputs: BTreeMap<String, ValueType>,
    outputs: BTreeMap<String, CheckedOutput>,
}

impl TaskNode for CodeNode {
    fn execute(
        &self,
        inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        let input_size = json_size(&inputs).context(MeasureInputsSnafu)?;
        if input_size > MAX_JSON_BYTES {
            return Err(execution_error(format!(
                "inputs exceed the {MAX_JSON_BYTES}-byte JSON limit"
            )));
        }
        for name in inputs.keys() {
            if !self.inputs.contains_key(name) {
                return Err(execution_error(format!("undeclared input `{name}`")));
            }
        }
        let mut budget = Budget::default();
        let mut activation = MapActivation::new();
        for (name, value_type) in &self.inputs {
            let name = name.as_str();
            let value = inputs
                .get(name)
                .ok_or_else(|| execution_error(format!("missing input `{name}`")))?;
            let converted = json_to_cel(value, value_type, &mut budget, "", 1)
                .map_err(|error| execution_error(format!("input `{name}`: {error}")))?;
            activation.insert(name, converted);
        }
        let mut outputs = Outputs::new();
        let mut output_size = 2usize;
        for (name, output) in &self.outputs {
            let name = name.as_str();
            let result = output.program.eval(&activation);
            if let CelValue::Error(error) = &result {
                Err(error.as_ref().clone()).context(EvaluateSnafu { output: name })?;
            }
            let converted = cel_to_json(&result, &output.value_type, &mut budget, "", 1)
                .map_err(|error| execution_error(format!("output `{name}`: {error}")))?;
            let encoded_name = json_size(name).context(MeasureOutputSnafu { output: name })?;
            let encoded_value =
                json_size(&converted).context(MeasureOutputSnafu { output: name })?;
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
impl NodePortContract for CodeNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: self
                .inputs
                .iter()
                .map(|(name, value_type)| PortSpec::owned(name, value_type.clone(), true))
                .collect(),
            outputs: self
                .outputs
                .iter()
                .map(|(name, output)| PortSpec::owned(name, output.value_type.clone(), true))
                .collect(),
        }
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
    let mut inputs = BTreeMap::new();
    for (name, descriptor) in &config.inputs {
        if !valid_identifier(name) {
            return Err(invalid(format!("invalid input name `{name}`")));
        }
        let (value_type, native_type) =
            parse_type(descriptor).map_err(|error| invalid(format!("input `{name}`: {error}")))?;
        env = env.with_variable(name, native_type);
        inputs.insert(name.clone(), value_type);
    }
    let code = config
        .code
        .as_object()
        .filter(|code| !code.is_empty())
        .ok_or_else(|| invalid("code must map output names to CEL expressions"))?;
    let mut outputs = BTreeMap::new();
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
            .context(CompileSnafu { output: name })?;
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
            .context(DepthSnafu { output: name })?;
        let program = env.program(&ast).context(CompileSnafu { output: name })?;
        outputs.insert(
            name.clone(),
            CheckedOutput {
                program,
                value_type,
            },
        );
    }
    mf_runtime::PreparedNode::new(
        CodeNode { inputs, outputs },
        mf_runtime::NodeMetadata::default(),
    )
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(factory) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn find_source<'a, T: Error + 'static>(error: &'a (dyn Error + 'static)) -> Option<&'a T> {
        let mut current = Some(error);
        while let Some(error) = current {
            if let Some(source) = error.downcast_ref::<T>() {
                return Some(source);
            }
            current = error.source();
        }
        None
    }

    #[test]
    fn preserves_typed_cel_compile_and_evaluation_errors() {
        let config = |expression| {
            serde_json::json!({
                "language": "cel", "inputs": {"x": "int"}, "code": {"result": expression}
            })
        };
        let error = factory(config("unknown + 1")).err().unwrap();
        assert!(find_source::<cel_core::CompileError>(&error).is_some());
        let prepared = factory(config("x / 0")).unwrap();
        let error = prepared
            .execution
            .as_task_node()
            .unwrap()
            .execute(
                Inputs::from([("x".into(), 1.into())]),
                &mut mf_runtime::ExecutionContext::default(),
            )
            .unwrap_err();
        assert!(find_source::<cel_core::EvalError>(&error).is_some());
        assert!(error.to_string().contains("output `result`"));
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
            (json!("uint"), "type must be a concrete"),
            (json!("usize"), "type must be a concrete"),
            (json!({"list": "float"}), "type must be a concrete"),
            (
                json!({"map": {"nullable": "string"}}),
                "type must be a concrete",
            ),
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
    fn explicitly_converts_text_to_inferred_numeric_outputs() {
        for (expression, expected_type, valid, invalid) in [
            (
                "int(text)",
                ValueType::Int64,
                vec![("42", json!(42)), ("-7", json!(-7))],
                vec!["", "invalid", "2.5", "9223372036854775808"],
            ),
            (
                "double(text)",
                ValueType::Float64,
                vec![("2.5", json!(2.5)), ("1e3", json!(1000.0))],
                vec!["", "invalid", "NaN", "Infinity"],
            ),
        ] {
            let node = factory(
                json!({"language":"cel", "inputs":{"text":"string"}, "code":{"number":expression}}),
            )
            .unwrap();
            assert_eq!(node.metadata.ports.outputs[0].value_type, expected_type);
            let execute = |text: &str| {
                node.execution.as_task_node().unwrap().execute(
                    Inputs::from([("text".into(), text.into())]),
                    &mut mf_runtime::ExecutionContext::default(),
                )
            };
            for (text, expected) in valid {
                assert_eq!(execute(text).unwrap().outputs["number"], expected);
            }
            for text in invalid {
                assert!(execute(text).is_err(), "{expression}: {text}");
            }
        }
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
