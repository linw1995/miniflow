use mf_runtime::{
    ExecutionContext, ExecutionScope, Inputs, LoopComparisonOperator, LoopConditionDefinition,
    LoopVariableDefinition, NodeBuildError, NodeConfig, NodeExecutionError, NodePortContract,
    NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, PreparedSubgraph, SubgraphNode,
    SubgraphStream, TypeCompatibility, ValueRef, ValueType, compare_json_numbers,
    loop_variable_types,
};
use mf_telemetry::{
    Count,
    event::{LoopPassOutcome, LoopStopReason, LoopSummary},
};
use serde_json::Value;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

pub const KIND: &str = mf_runtime::LOOP_KIND;

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct Config {
    body: PreparedSubgraph,
    max_iterations: u16,
    #[serde(default)]
    inputs: BTreeMap<String, ValueType>,
    variables: Vec<LoopVariableDefinition>,
    #[serde(default)]
    until: Option<LoopConditionDefinition>,
    #[serde(default)]
    stream_outputs: BTreeMap<String, mf_runtime::OutputReferenceDefinition>,
}

fn plain(_config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    mf_runtime::NodeInvalidSubgraphSnafu {
        message: "workflow.loop requires a prepared `body` binding",
    }
    .fail()
}

#[derive(Debug, Snafu)]
pub enum LoopBuildError {
    #[snafu(display("Loop stream names must be nonblank and distinct from variables and inputs"))]
    InvalidStreamName,
    #[snafu(display("max_iterations must be in 1..=1000"))]
    InvalidMaximum,
    #[snafu(display("invalid Loop variables: {source}"))]
    Variables {
        source: mf_runtime::LoopVariableError,
    },
    #[snafu(display("invalid Loop input name `{name}`"))]
    InvalidInputName { name: String },
    #[snafu(display("Loop input `{name}` collides with a variable"))]
    DuplicateInput { name: String },
    #[snafu(display("invalid initial value for Loop variable `{variable}`: {source}"))]
    InitialValue {
        variable: String,
        source: mf_runtime::TypeMismatch,
    },
    #[snafu(display("unknown condition variable `{variable}`"))]
    UnknownConditionVariable { variable: String },
    #[snafu(display("condition comparison value must be scalar"))]
    NonScalarConditionValue,
    #[snafu(display("ordering condition requires numeric operands"))]
    NonNumericOrdering,
    #[snafu(display("condition variable must have a scalar-compatible type"))]
    NonScalarConditionVariable,
    #[snafu(display("condition literal conflicts with variable type"))]
    IncompatibleConditionLiteral,
    #[snafu(display("Loop body must contain at least one node"))]
    EmptyBody,
    #[snafu(display("Loop body output `{variable}` must name a mutable variable"))]
    UnknownBodyOutput { variable: String },
    #[snafu(display("duplicate Loop body output `{variable}`"))]
    DuplicateBodyOutput { variable: String },
    #[snafu(display(
        "Loop body output `{variable}` cannot update a variable written by an immediate assignment"
    ))]
    ConflictingBodyOutput { variable: String },
    #[snafu(display("Loop body output `{variable}` of type {actual} cannot update {expected}"))]
    BodyOutputType {
        variable: String,
        actual: ValueType,
        expected: ValueType,
    },
    #[snafu(display(
        "Loop body scope bindings must match its variables, inputs, and identity state outputs"
    ))]
    InvalidScopeBindings,
}

impl From<LoopBuildError> for NodeBuildError {
    fn from(source: LoopBuildError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

struct LoopTypes {
    state: BTreeMap<String, ValueType>,
    inputs: BTreeMap<String, ValueType>,
    combined: BTreeMap<String, ValueType>,
    initial: BTreeMap<String, ValueRef>,
}

/// Validate the owning node's options against the prepared stream boundary.
fn validate_options<'a>(
    max_iterations: u16,
    variables: &[LoopVariableDefinition],
    inputs: BTreeMap<String, ValueType>,
    until: Option<&LoopConditionDefinition>,
    stream_names: impl IntoIterator<Item = &'a str>,
) -> Result<LoopTypes, LoopBuildError> {
    ensure!(
        (1..=mf_runtime::MAX_LOOP_ITERATIONS).contains(&max_iterations),
        InvalidMaximumSnafu
    );
    let state = loop_variable_types(variables).context(VariablesSnafu)?;
    for name in inputs.keys() {
        ensure!(
            !name.trim().is_empty() && name != "index",
            InvalidInputNameSnafu { name }
        );
        ensure!(!state.contains_key(name), DuplicateInputSnafu { name });
    }
    let mut combined = inputs.clone();
    combined.extend(
        state
            .iter()
            .map(|(name, value_type)| (name.clone(), value_type.clone())),
    );
    let mut initial = BTreeMap::new();
    for variable in variables {
        if let Some(value) = &variable.initial {
            let value_type = &state[&variable.name];
            value_type
                .validate_value(value)
                .context(InitialValueSnafu {
                    variable: &variable.name,
                })?;
            initial.insert(variable.name.clone(), ValueRef::from(value.clone()));
        }
    }
    ensure!(
        stream_names
            .into_iter()
            .all(|name| !name.trim().is_empty() && !combined.contains_key(name)),
        InvalidStreamNameSnafu
    );
    if let Some(condition) = until {
        let value_type =
            state
                .get(&condition.variable)
                .with_context(|| UnknownConditionVariableSnafu {
                    variable: &condition.variable,
                })?;
        ensure!(
            !condition.value.is_array() && !condition.value.is_object(),
            NonScalarConditionValueSnafu
        );
        let numeric = matches!(
            value_type,
            ValueType::Any
                | ValueType::Number
                | ValueType::Int64
                | ValueType::Uint64
                | ValueType::Usize
                | ValueType::Float32
                | ValueType::Float64
        );
        let order = matches!(
            condition.operator,
            LoopComparisonOperator::Gt
                | LoopComparisonOperator::Gte
                | LoopComparisonOperator::Lt
                | LoopComparisonOperator::Lte
        );
        ensure!(
            !order || (numeric && condition.value.is_number()),
            NonNumericOrderingSnafu
        );
        if !order {
            ensure!(
                !matches!(
                    value_type,
                    ValueType::Array
                        | ValueType::Object
                        | ValueType::Flow
                        | ValueType::List(_)
                        | ValueType::Map(_)
                ),
                NonScalarConditionVariableSnafu
            );
            let compatible = match value_type {
                ValueType::Any => true,
                ValueType::Number
                | ValueType::Int64
                | ValueType::Uint64
                | ValueType::Usize
                | ValueType::Float32
                | ValueType::Float64 => condition.value.is_number(),
                ValueType::Null => condition.value.is_null(),
                ValueType::Boolean => condition.value.is_boolean(),
                ValueType::String => condition.value.is_string(),
                ValueType::Nullable(_) => value_type.validate_value(&condition.value).is_ok(),
                ValueType::Array
                | ValueType::Object
                | ValueType::Flow
                | ValueType::List(_)
                | ValueType::Map(_) => false,
            };
            ensure!(compatible, IncompatibleConditionLiteralSnafu);
        }
    }
    Ok(LoopTypes {
        state,
        inputs,
        combined,
        initial,
    })
}

fn factory(
    id: &str,
    config: Value,
    bodies: BTreeMap<String, PreparedSubgraph>,
) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let Config {
        body,
        max_iterations,
        inputs,
        variables,
        until,
        stream_outputs,
    } = Config::from_config(config, bodies)?;
    let types = validate_options(
        max_iterations,
        &variables,
        inputs,
        until.as_ref(),
        stream_outputs
            .keys()
            .map(String::as_str)
            .chain(body.stream_outputs.iter().map(|port| port.name.as_ref())),
    )?;
    if let Some(relations) = &body.relations {
        ensure!(
            relations.scope.as_ref().is_some_and(|scope| {
                scope.values == types.combined
                    && scope.state_outputs
                        == types
                            .state
                            .keys()
                            .map(|name| (name.clone(), name.clone()))
                            .collect()
            }),
            InvalidScopeBindingsSnafu
        );
    }
    ensure!(!body.nodes.is_empty(), EmptyBodySnafu);
    let mut output_names = BTreeSet::new();
    for port in &body.outputs {
        let variable = port.name.as_ref();
        let expected = types
            .state
            .get(variable)
            .context(UnknownBodyOutputSnafu { variable })?;
        ensure!(
            output_names.insert(variable),
            DuplicateBodyOutputSnafu { variable }
        );
        ensure!(
            port.value_type.compatibility_with(expected) != TypeCompatibility::Incompatible,
            BodyOutputTypeSnafu {
                variable,
                actual: port.value_type.clone(),
                expected: expected.clone(),
            }
        );
        ensure!(
            body.relations
                .as_ref()
                .and_then(|relations| relations.scope.as_ref())
                .is_none_or(|scope| scope.writes.values().all(|target| target != variable)),
            ConflictingBodyOutputSnafu { variable }
        );
    }
    let node = LoopNode {
        id: id.into(),
        scope_key: body.scope_key(id, "body"),
        max_iterations,
        until,
        types: types.state,
        inputs: types.inputs,
        initial: types.initial,
        body,
    };
    mf_runtime::PreparedNode::subgraph(node, mf_runtime::NodeMetadata::default())
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        factory: mf_runtime::NodeFactory::Subgraph {
            flow_parameters: Config::flow_parameters,
            plain,
            prepared: factory,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::NodeRegistry;
    use serde_json::json;
    use std::error::Error;

    fn body() -> PreparedSubgraph {
        PreparedSubgraph::new(
            vec![mf_telemetry::event::NodeIdentity {
                id: "identity".into(),
                kind: "builtin.identity".into(),
                path: Vec::new(),
            }],
            Vec::new(),
            |_| Ok(Outputs::new()),
        )
    }

    fn body_relations(input: &str) -> mf_runtime::GraphRelations {
        mf_runtime::GraphRelations {
            scope: Some(mf_runtime::ScopeRelations {
                input: input.into(),
                values: BTreeMap::from([("x".into(), ValueType::Int64)]),
                state_outputs: BTreeMap::from([("x".into(), "x".into())]),
                writes: BTreeMap::new(),
                exits: Default::default(),
            }),
            ..Default::default()
        }
    }

    fn config() -> Value {
        json!({
            "max_iterations": 1,
            "variables": [{"name":"x", "type":"int"}],
            "body": {"version":"2026-10-28", "dependencies":{}, "nodes":[]},
        })
    }

    fn build_error(config: Value, body: PreparedSubgraph) -> NodeBuildError {
        factory("repeat", config, BTreeMap::from([("body".into(), body)]))
            .err()
            .unwrap()
    }

    fn business_error(error: &NodeBuildError) -> &LoopBuildError {
        let NodeBuildError::FactoryFailed { source } = error else {
            panic!("expected owning-node business error, got {error}");
        };
        source.downcast_ref().unwrap()
    }

    #[test]
    fn loop_assignment_refreshes_the_prepared_scope_input_or_manual_default() {
        for scope_input in [None, Some("custom-loop-input")] {
            let source_id = scope_input.unwrap_or(mf_runtime::LOOP_SOURCE_ID);
            let types = BTreeMap::from([("x".into(), ValueType::Int64)]);
            let source = mf_runtime::prepared_scope_source(source_id, &types)
                .into_task()
                .unwrap();
            let assign = mf_runtime::prepared_loop_assign("assign", "x", ValueType::Int64)
                .into_task()
                .unwrap();
            let mut prepared = PreparedSubgraph::new(body().nodes, Vec::new(), move |state| {
                mf_runtime::execute_node_in_context(&source, &[], state)?;
                mf_runtime::execute_node_in_context(
                    &assign,
                    &[mf_runtime::ExecutionDependency {
                        exclusive: false,
                        input: Some("value"),
                        source_node: source_id,
                        source_output: "index",
                        source_key: mf_runtime::output_id(source_id, "index").into(),
                    }],
                    state,
                )?;
                let value = state
                    .select_output("current", source_id, "x", false)?
                    .unwrap();
                assert_eq!(value, json!(0));
                if source_id != mf_runtime::LOOP_SOURCE_ID {
                    assert!(
                        state
                            .output(&mf_runtime::output_id(mf_runtime::LOOP_SOURCE_ID, "x"))
                            .is_err()
                    );
                }
                Ok(Outputs::new())
            });
            if let Some(input) = scope_input {
                prepared = prepared.with_relations(body_relations(input));
            }
            let node = LoopNode {
                id: "repeat".into(),
                scope_key: prepared.scope_key("repeat", "body"),
                max_iterations: 1,
                until: None,
                types,
                inputs: BTreeMap::new(),
                initial: BTreeMap::new(),
                body: prepared,
            };
            let result = SubgraphNode::execute(
                &node,
                Inputs::from([("x".into(), 7.into())]),
                &mut ExecutionContext::default(),
                &mut mf_runtime::DiscardSink,
            )
            .unwrap();
            assert_eq!(result.outputs["x"], json!(0));
        }
    }

    #[test]
    fn loop_invocation_paths_use_the_actual_child_edge_key() {
        for compiled in [false, true] {
            for mapped in [false, true] {
                let key = mf_runtime::ScopePathEntry::binding_key("mapped-owner", "selected");
                let expected = if !compiled {
                    "repeat".to_owned()
                } else if mapped {
                    key.clone()
                } else {
                    mf_runtime::ScopePathEntry::binding_key("repeat", "body")
                };
                let mut body = PreparedSubgraph::new(body().nodes, Vec::new(), move |state| {
                    let path = state.invocation_path();
                    assert_eq!(path.len(), 1);
                    assert_eq!(path[0].node_id(), expected);
                    assert_eq!(path[0].index(), Count::ZERO);
                    assert_eq!(state.current_node(), Some("repeat"));
                    Ok(Outputs::new())
                });
                if compiled {
                    let mut relations = body_relations("input");
                    relations.key = mapped.then_some(key);
                    body = body.with_relations(relations);
                }
                let node = mf_runtime::FlowNode::new(
                    "repeat",
                    factory("repeat", config(), BTreeMap::from([("body".into(), body)])).unwrap(),
                )
                .into_task()
                .unwrap();
                let mut context = ExecutionContext::default();
                context
                    .with_node("repeat".into(), |context| {
                        let result = node
                            .node
                            .execute(Inputs::from([("x".into(), 7.into())]), context);
                        assert_eq!(context.current_node(), Some("repeat"));
                        assert!(context.invocation_path().is_empty());
                        result
                    })
                    .unwrap();
                assert!(context.current_node().is_none());
            }
        }
    }

    #[test]
    fn loop_factory_checks_compiled_scope_bindings_before_execution() {
        let prepared = body().with_relations(body_relations("custom-input"));
        assert!(
            factory(
                "repeat",
                config(),
                BTreeMap::from([("body".into(), prepared)]),
            )
            .is_ok()
        );
        let mut invalid = vec![mf_runtime::GraphRelations::default()];
        for mismatch in 0..7 {
            let mut relations = body_relations("custom-input");
            let scope = relations.scope.as_mut().unwrap();
            match mismatch {
                0 => scope.values.clear(),
                1 => {
                    scope.values.insert("x".into(), ValueType::String);
                }
                2 => {
                    scope.values.insert("extra".into(), ValueType::Any);
                }
                3 => scope.state_outputs.clear(),
                4 => {
                    scope.state_outputs.insert("x".into(), "renamed".into());
                }
                5 => {
                    scope.state_outputs.insert("extra".into(), "extra".into());
                }
                6 => {
                    scope.values.insert("extra".into(), ValueType::Int64);
                    scope.state_outputs.insert("extra".into(), "extra".into());
                }
                _ => unreachable!(),
            }
            invalid.push(relations);
        }
        for relations in invalid {
            let prepared = PreparedSubgraph::new(body().nodes, Vec::new(), |_| {
                panic!("invalid scope bindings must be rejected before execution")
            })
            .with_relations(relations);
            let error = build_error(config(), prepared);
            assert!(matches!(
                business_error(&error),
                LoopBuildError::InvalidScopeBindings
            ));
        }
    }

    #[test]
    fn loop_factory_rejects_missing_and_unexpected_body_handles() {
        let config = json!({
            "max_iterations": 1,
            "variables": [{"name":"x", "type":"int"}],
            "body": {"version":"2026-10-28", "dependencies":{}, "nodes":[]},
        });
        for bodies in [
            BTreeMap::new(),
            BTreeMap::from([("unexpected".into(), body())]),
        ] {
            let error = factory("repeat", config.clone(), bodies).err().unwrap();
            assert!(error.to_string().contains("prepared Flow field"), "{error}");
        }
    }

    #[test]
    fn loop_factory_uses_prepared_boundary_not_raw_body() {
        let mut config = config();
        config["body"] = json!({"not": "a workflow", "outputs": ["poison"]});
        assert!(factory("repeat", config, BTreeMap::from([("body".into(), body())])).is_ok());

        let mut empty = body();
        empty.nodes.clear();
        let error = build_error(self::config(), empty);
        assert!(matches!(business_error(&error), LoopBuildError::EmptyBody));

        let mut terminal = body();
        terminal
            .outputs
            .push(PortSpec::new("result", ValueType::Any, true));
        let error = build_error(self::config(), terminal);
        assert!(matches!(
            business_error(&error),
            LoopBuildError::UnknownBodyOutput { .. }
        ));
    }

    #[test]
    fn loop_factory_validates_prepared_output_contracts() {
        let mut duplicate = body();
        duplicate.outputs = vec![
            PortSpec::new("x", ValueType::Int64, true),
            PortSpec::new("x", ValueType::Int64, true),
        ];
        let error = build_error(config(), duplicate);
        assert!(matches!(
            business_error(&error),
            LoopBuildError::DuplicateBodyOutput { .. }
        ));

        for (outputs, required, expected) in [
            (Outputs::new(), true, "required update"),
            (
                Outputs::from([("unexpected".into(), 1.into())]),
                false,
                "undeclared update",
            ),
            (
                Outputs::from([("x".into(), "wrong".into())]),
                true,
                "invalid Loop body update",
            ),
        ] {
            let prepared = PreparedSubgraph::new(
                body().nodes,
                vec![PortSpec::new("x", ValueType::Any, required)],
                move |_| Ok(outputs.clone()),
            );
            let node = factory(
                "repeat",
                config(),
                BTreeMap::from([("body".into(), prepared)]),
            )
            .unwrap()
            .execution
            .into_task_node()
            .unwrap();
            let error = node
                .execute(
                    Inputs::from([("x".into(), 0.into())]),
                    &mut ExecutionContext::default(),
                )
                .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn loop_factory_preserves_typed_variable_sources_and_codec_errors() {
        let mut config = config();
        config["variables"][0]["type"] = json!("not-a-type");
        let error = build_error(config, body());
        let business = business_error(&error);
        assert!(matches!(
            business,
            LoopBuildError::Variables {
                source: mf_runtime::LoopVariableError::VariableType { .. }
            }
        ));
        let variables = business.source().unwrap();
        assert!(
            variables
                .downcast_ref::<mf_runtime::LoopVariableError>()
                .is_some()
        );
        assert!(variables.source().is_some());

        let mut config = self::config();
        config["max_iterations"] = json!("wrong JSON type");
        assert!(matches!(
            build_error(config, body()),
            NodeBuildError::InvalidConfiguration { .. }
        ));
    }

    #[test]
    fn loop_factory_enforces_maximum_variables_and_stream_names() {
        for maximum in [0, mf_runtime::MAX_LOOP_ITERATIONS + 1] {
            let mut config = config();
            config["max_iterations"] = json!(maximum);
            let error = build_error(config, body());
            assert!(matches!(
                business_error(&error),
                LoopBuildError::InvalidMaximum
            ));
        }
        for variables in [
            json!([]),
            json!([{"name":" ", "type":"int"}]),
            json!([{"name":"index", "type":"int"}]),
            json!([{"name":"x", "type":"int"}, {"name":"x", "type":"int"}]),
        ] {
            let mut config = config();
            config["variables"] = variables;
            let error = build_error(config, body());
            assert!(matches!(
                business_error(&error),
                LoopBuildError::Variables { .. }
            ));
        }
        for name in [" ", "x"] {
            let mut prepared = body();
            prepared
                .stream_outputs
                .push(PortSpec::owned(name, ValueType::Any, true));
            let error = build_error(config(), prepared);
            assert!(matches!(
                business_error(&error),
                LoopBuildError::InvalidStreamName
            ));

            let mut config = config();
            config["stream_outputs"] = json!({(name): {"node":"identity", "port":"value"}});
            let error = build_error(config, body());
            assert!(matches!(
                business_error(&error),
                LoopBuildError::InvalidStreamName
            ));
        }
    }

    #[test]
    fn loop_factory_checks_condition_semantics() {
        for (descriptor, condition, expected) in [
            (
                json!("int"),
                json!({"variable":"missing", "operator":"eq", "value":0}),
                "unknown",
            ),
            (
                json!("int"),
                json!({"variable":"x", "operator":"eq", "value":[]}),
                "value",
            ),
            (
                json!("string"),
                json!({"variable":"x", "operator":"gt", "value":0}),
                "ordering",
            ),
            (
                json!("int"),
                json!({"variable":"x", "operator":"gte", "value":"0"}),
                "ordering",
            ),
            (
                json!("array"),
                json!({"variable":"x", "operator":"eq", "value":0}),
                "variable",
            ),
            (
                json!({"list": "int"}),
                json!({"variable":"x", "operator":"eq", "value":0}),
                "variable",
            ),
            (
                json!({"map": "int"}),
                json!({"variable":"x", "operator":"eq", "value":0}),
                "variable",
            ),
            (
                json!("int"),
                json!({"variable":"x", "operator":"ne", "value":"0"}),
                "literal",
            ),
            (
                json!({"nullable": "int"}),
                json!({"variable":"x", "operator":"eq", "value":"0"}),
                "literal",
            ),
        ] {
            let mut config = config();
            config["variables"][0]["type"] = descriptor;
            config["until"] = condition;
            let error = build_error(config, body());
            assert!(
                matches!(
                    (business_error(&error), expected),
                    (LoopBuildError::UnknownConditionVariable { .. }, "unknown")
                        | (LoopBuildError::NonScalarConditionValue, "value")
                        | (LoopBuildError::NonNumericOrdering, "ordering")
                        | (LoopBuildError::NonScalarConditionVariable, "variable")
                        | (LoopBuildError::IncompatibleConditionLiteral, "literal")
                ),
                "{error}"
            );
        }
        for (descriptor, value) in [
            (json!("int"), json!(0)),
            (json!("string"), json!("done")),
            (json!("bool"), json!(true)),
            (json!("null"), Value::Null),
            (json!({"nullable": "int"}), Value::Null),
            (json!({"nullable": "int"}), json!(0)),
            (json!("any"), json!(true)),
        ] {
            let mut config = config();
            config["variables"][0]["type"] = descriptor;
            config["until"] = json!({"variable":"x", "operator":"eq", "value":value});
            assert!(factory("repeat", config, BTreeMap::from([("body".into(), body())])).is_ok());
        }
    }

    #[test]
    fn registers_loop_factory_and_requires_a_prepared_body_and_valid_config() {
        let registration = NodeRegistry::from_inventory().unwrap().get(KIND).unwrap();
        let config = json!({
            "body": {"version":"2026-10-28", "dependencies":{}, "nodes":[]},
            "max_iterations": {"nodes": []}, "variables": {"nodes": []},
            "until": {"nodes": []}, "stream_outputs": {"nodes": []},
        });
        assert_eq!(
            registration.flow_parameters(&config),
            BTreeMap::from([("body".into(), config["body"].clone())])
        );
        for config in [json!({}), json!({"body": null})] {
            assert!(registration.flow_parameters(&config).is_empty());
        }
        assert!(
            registration
                .instantiate(json!({"unexpected": true}))
                .is_err()
        );
        assert!(registration.instantiate(json!({})).is_err());
        let config = json!({
            "max_iterations":1,
            "variables":[{"name":"x", "type":"int"}],
            "body": {"version":"2026-10-28", "dependencies":{}, "nodes":[]}
        });
        let mut invalid = config.clone();
        invalid["unexpected"] = json!(true);
        for (config, valid) in [(config, true), (invalid, false)] {
            let prepared = registration.instantiate_subgraphs(
                "repeat",
                config,
                BTreeMap::from([("body".into(), body())]),
            );
            assert_eq!(prepared.is_ok(), valid);
        }
    }
}

fn structural_error(message: impl Into<String>) -> NodeExecutionError {
    mf_runtime::NodeExecutionFailedSnafu {
        message: message.into(),
    }
    .build()
}

struct LoopNode {
    id: String,
    scope_key: String,
    max_iterations: u16,
    until: Option<LoopConditionDefinition>,
    types: BTreeMap<String, ValueType>,
    inputs: BTreeMap<String, ValueType>,
    initial: BTreeMap<String, ValueRef>,
    body: PreparedSubgraph,
}

impl NodePortContract for LoopNode {
    fn ports(&self) -> NodePorts {
        let mut inputs: Vec<_> = self
            .types
            .iter()
            .map(|(name, value_type)| {
                PortSpec::owned(name, value_type.clone(), !self.initial.contains_key(name))
            })
            .collect();
        inputs.extend(
            self.inputs
                .iter()
                .map(|(name, value_type)| PortSpec::owned(name, value_type.clone(), true)),
        );
        let outputs = self
            .types
            .iter()
            .map(|(name, value_type)| PortSpec::owned(name, value_type.clone(), true))
            .collect();
        NodePorts { inputs, outputs }
    }
}

#[derive(Debug, Snafu)]
enum LoopUpdateError {
    #[snafu(display("Loop body returned undeclared update `{variable}`"))]
    UndeclaredUpdate { variable: String },
    #[snafu(display("Loop body did not return required update `{variable}`"))]
    MissingUpdate { variable: String },
    #[snafu(display("invalid Loop body update `{variable}`: {source}"))]
    UpdateType {
        variable: String,
        source: mf_runtime::TypeMismatch,
    },
}

impl From<LoopUpdateError> for NodeExecutionError {
    fn from(source: LoopUpdateError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

impl LoopNode {
    fn validate_updates(&self, updates: &Outputs) -> Result<(), NodeExecutionError> {
        for port in &self.body.outputs {
            ensure!(
                !port.required || updates.contains_key(port.name.as_ref()),
                MissingUpdateSnafu {
                    variable: port.name.as_ref(),
                }
            );
        }
        for (variable, value) in updates {
            ensure!(
                self.body.outputs.iter().any(|port| port.name == *variable),
                UndeclaredUpdateSnafu { variable }
            );
            self.types[variable]
                .validate_shared(value)
                .context(UpdateTypeSnafu { variable })?;
        }
        Ok(())
    }
}

#[derive(Debug, Snafu)]
#[snafu(display("Loop `{node}` pass {index}: {source}"))]
struct LoopPassFailure {
    node: String,
    index: usize,
    source: mf_runtime::WorkflowRunError,
}

impl From<LoopPassFailure> for NodeExecutionError {
    fn from(source: LoopPassFailure) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

impl SubgraphNode for LoopNode {
    fn body(&self) -> &PreparedSubgraph {
        &self.body
    }

    fn supports_success_dependencies(&self) -> bool {
        false
    }

    fn execute(
        &self,
        inputs: Inputs,
        ctx: &mut ExecutionContext,
        sink: &mut dyn mf_runtime::EmissionSink,
    ) -> Result<NodeResult, NodeExecutionError> {
        let mut remaining = inputs;
        let mut variables = Inputs::new();
        for name in self.inputs.keys() {
            let value = remaining
                .remove(name)
                .ok_or_else(|| structural_error(format!("missing Loop input `{name}`")))?;
            variables.insert(name.clone(), value);
        }
        for name in self.types.keys() {
            let value = match remaining.remove(name) {
                Some(value) => value,
                None => self
                    .initial
                    .get(name)
                    .cloned()
                    .ok_or_else(|| structural_error(format!("missing Loop value `{name}`")))?,
            };
            variables.insert(name.clone(), value);
        }
        if let Some(name) = remaining.keys().next() {
            return Err(structural_error(format!("unexpected Loop input `{name}`")));
        }
        let mut stream = SubgraphStream::new(self.id.as_str(), ctx, sink);
        let mut pass_count = 0;
        let mut reason = LoopStopReason::Maximum;
        for index in 0..usize::from(self.max_iterations) {
            let scope = ExecutionScope::new(
                &self.scope_key,
                self.body
                    .scope_input()
                    .map_or(mf_runtime::LOOP_SOURCE_ID, mf_runtime::DefinitionId::as_str),
                index,
                std::mem::take(&mut variables),
                self.types.clone(),
            )?;
            let (updates, updated, exited) = ctx
                .run_scope(scope, |state| {
                    let path = state.invocation_path();
                    let mut pass = state.observation_mut().and_then(|run| {
                        let pass = run.begin_loop_pass(path.clone());
                        if pass.is_none() && run.supports_loops() {
                            run.loop_pass_started(path.clone());
                        }
                        pass
                    });
                    let _guard = pass.as_ref().map(|pass| pass.enter());
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let updates = self.body.execute_with_sink(state, &mut stream)?;
                        if !state.scope_exit_requested() {
                            self.validate_updates(&updates).context(
                                mf_runtime::NodeExecutionSnafu {
                                    definition_id: self.id.as_str(),
                                },
                            )?;
                        }
                        Ok(updates)
                    }));
                    let outcome = if !matches!(&result, Ok(Ok(_))) {
                        LoopPassOutcome::Failed
                    } else if state.scope_exit_requested() {
                        LoopPassOutcome::Exit
                    } else {
                        LoopPassOutcome::Completed
                    };
                    if let Some(pass) = &mut pass {
                        pass.finish(outcome);
                    } else if let Some(run) =
                        state.observation_mut().filter(|run| run.supports_loops())
                    {
                        run.loop_pass_finished(path, outcome);
                    }
                    match result {
                        Ok(result) => result,
                        Err(payload) => std::panic::resume_unwind(payload),
                    }
                })
                .context(LoopPassFailureSnafu {
                    node: self.id.clone(),
                    index,
                })?;
            variables = updated;
            pass_count = index + 1;
            if exited {
                reason = LoopStopReason::Exit;
                break;
            }
            variables.extend(updates);
            if let Some(condition) = &self.until
                && condition_matches(condition, &variables)?
            {
                reason = LoopStopReason::Condition;
                break;
            }
        }
        let mut outputs = Outputs::new();
        for name in self.types.keys() {
            let value = variables
                .remove(name)
                .ok_or_else(|| structural_error(format!("missing Loop state `{name}`")))?;
            outputs.insert(name.clone(), value);
        }
        Ok(NodeResult {
            outputs,
            loop_summary: Some(LoopSummary {
                pass_count: Count::try_from(pass_count as i64).expect("Loop pass count is bounded"),
                reason,
            }),
            ..NodeResult::default()
        })
    }
}

fn condition_matches(
    condition: &LoopConditionDefinition,
    variables: &Outputs,
) -> Result<bool, NodeExecutionError> {
    let actual = variables.get(&condition.variable).ok_or_else(|| {
        structural_error(format!("missing Loop variable `{}`", condition.variable))
    })?;
    let expected = &condition.value;
    if actual.is_array() || actual.is_object() {
        return Err(structural_error(format!(
            "Loop condition variable `{}` must be scalar",
            condition.variable
        )));
    }
    let order = match (actual.as_number(), expected.as_number()) {
        (Some(left), Some(right)) => Some(compare_json_numbers(left, right)),
        _ => None,
    };
    let result = match condition.operator {
        LoopComparisonOperator::Eq => {
            order.map_or(actual == expected, |order| order == Ordering::Equal)
        }
        LoopComparisonOperator::Ne => {
            order.map_or(actual != expected, |order| order != Ordering::Equal)
        }
        LoopComparisonOperator::Gt => order == Some(Ordering::Greater),
        LoopComparisonOperator::Gte => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
        LoopComparisonOperator::Lt => order == Some(Ordering::Less),
        LoopComparisonOperator::Lte => matches!(order, Some(Ordering::Less | Ordering::Equal)),
    };
    if matches!(
        condition.operator,
        LoopComparisonOperator::Gt
            | LoopComparisonOperator::Gte
            | LoopComparisonOperator::Lt
            | LoopComparisonOperator::Lte
    ) && order.is_none()
    {
        return Err(structural_error(format!(
            "Loop condition variable `{}` requires numeric operands",
            condition.variable
        )));
    }
    Ok(result)
}
