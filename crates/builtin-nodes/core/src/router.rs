use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodePortContract, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, PreparedNode,
    TaskNode, ValueRef, ValueType, WorkflowArguments, WorkflowRunError,
};
use serde::Deserialize;
use serde_json::{Value, json};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::collections::{BTreeMap, BTreeSet};

pub const KIND: &str = "builtin.router";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    routes: Option<Vec<String>>,
    mode: Option<Mode>,
    flow_field: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Execute,
}

#[derive(Debug, Snafu)]
pub enum RouterBuildError {
    #[snafu(display("Router requires either routes or mode `execute`, but not both"))]
    Mode,
    #[snafu(display("Router requires at least one route"))]
    Empty,
    #[snafu(display("Blank, reserved, or duplicate router route `{route}`"))]
    Route { route: String },
    #[snafu(display("Router flow_field must be nonblank"))]
    FlowField,
}

impl From<RouterBuildError> for NodeBuildError {
    fn from(source: RouterBuildError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

struct Router {
    routes: Vec<String>,
}

impl NodePortContract for Router {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("route", ValueType::String, true)],
            outputs: self
                .routes
                .iter()
                .map(String::as_str)
                .chain(["else"])
                .map(|name| PortSpec::owned(name, ValueType::Boolean, false))
                .collect(),
        }
    }
}

impl TaskNode for Router {
    fn execute(
        &self,
        inputs: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let route = &inputs["route"];
        let requested = route.as_str().expect("validated route string");
        let selected = self
            .routes
            .iter()
            .find(|route| route.as_str() == requested)
            .map_or("else", String::as_str);
        Ok(NodeResult {
            outputs: Outputs::from([(
                selected.to_owned(),
                ValueRef::from(true).inherit_secrets([route]),
            )]),
            skipped: self
                .routes
                .iter()
                .map(String::as_str)
                .chain(["else"])
                .filter(|route| *route != selected)
                .map(str::to_owned)
                .collect(),
            ..Default::default()
        })
    }
}

struct ExecuteRouter {
    flow_field: Option<String>,
}

impl NodePortContract for ExecuteRouter {
    fn ports(&self) -> NodePorts {
        let flows = if self.flow_field.is_some() {
            ValueType::Map(Box::new(ValueType::Object))
        } else {
            ValueType::Map(Box::new(ValueType::Flow))
        };
        NodePorts {
            inputs: vec![
                PortSpec::new("route", ValueType::String, true),
                PortSpec::new("flows", flows, true),
                PortSpec::new("arguments", ValueType::Object, true),
            ],
            outputs: vec![PortSpec::new("result", ValueType::Object, true)],
        }
    }
}

#[derive(Debug, Snafu)]
pub enum RouterExecutionError {
    #[snafu(display("Router `{node}` has no Flow route `{route}`"))]
    UnknownRoute { node: String, route: String },
    #[snafu(display("Router `{node}` route `{route}` is not a native Flow value"))]
    InvalidFlow { node: String, route: String },
    #[snafu(display("Router `{node}` Flow `{route}`: {source}"))]
    Child {
        node: String,
        route: String,
        source: WorkflowRunError,
    },
}

impl From<RouterExecutionError> for NodeExecutionError {
    fn from(source: RouterExecutionError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

/// Recover only an explicit ordinary node business failure. In particular, Context
/// is not a business error: invocation scope, resource and budget failures use it.
/// Only known invocation wrappers are unwrapped; unknown plugin errors propagate.
/// This keeps new runtime contracts fail-closed.
fn is_business_failure(error: &WorkflowRunError) -> bool {
    // Selection and arguments are caller data. Their validation happens before any
    // child body effect, so an invalid invocation is a business result rather than
    // an infrastructure failure. `Context` and resource errors stay fatal.
    if matches!(error, WorkflowRunError::WorkflowInputs { .. }) {
        return true;
    }
    let WorkflowRunError::NodeExecution { source, .. } = error else {
        return false;
    };
    match source {
        NodeExecutionError::ExecutionFailed { .. } => true,
        NodeExecutionError::PluginFailed { source } => {
            if let Some(error) = source.downcast_ref::<WorkflowRunError>() {
                return is_business_failure(error);
            }
            if let Some(error) = source.downcast_ref::<crate::call::CallFailure>() {
                return is_business_failure(&error.source);
            }
            match source.downcast_ref::<crate::CallInputError>() {
                Some(crate::CallInputError::Execution { source, .. }) => {
                    is_business_failure(source)
                }
                _ => false,
            }
        }
        _ => false,
    }
}

impl TaskNode for ExecuteRouter {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        if let Some(error) = context.cancellation().failure() {
            return Err(error.into());
        }
        let id = context.current_node().unwrap_or(KIND).to_owned();
        let route_value = &inputs["route"];
        let route = route_value.as_str().expect("validated route string");
        // Error displays are captured verbatim by telemetry and failed snapshots.
        // Store only the safe diagnostic label, never a private selection string.
        let display_route = if route_value.is_secret() {
            "[REDACTED]"
        } else {
            route
        };
        // Keep the selected ValueRef, not just its Flow payload: selection itself
        // can be secret, independently of the callable's arguments and outputs.
        let selected = inputs["flows"].get(route).context(UnknownRouteSnafu {
            node: &id,
            route: display_route,
        })?;
        let selected = if let Some(field) = &self.flow_field {
            selected.get(field).context(InvalidFlowSnafu {
                node: &id,
                route: display_route,
            })?
        } else {
            selected
        };
        let flow = selected.as_flow().context(InvalidFlowSnafu {
            node: &id,
            route: display_route,
        })?;
        let arguments = inputs["arguments"]
            .as_object()
            .expect("validated arguments object")
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect();
        // invoke owns validation, origin, isolation and the invocation lifecycle.
        // Router deliberately drains emissions and exposes only terminal outputs.
        let result = flow
            .invoke(&id, WorkflowArguments(arguments), context)
            .context(ChildSnafu {
                node: &id,
                route: display_route,
            });
        if let Some(error) = context.cancellation().failure() {
            return Err(error.into());
        }
        let value = match result {
            Ok(outputs) => ValueRef::object([(
                "outputs".into(),
                ValueRef::object(
                    outputs
                        .into_iter()
                        .map(|(name, value)| (name.into(), value)),
                ),
            )]),
            Err(error)
                if matches!(&error, RouterExecutionError::Child { source, .. } if is_business_failure(source)) =>
            {
                // Error strings can contain private arguments even when no input
                // was explicitly marked secret. Never publish them as public data.
                ValueRef::from(json!({"error": error.to_string()})).into_secret()
            }
            Err(error) => return Err(error.into()),
        }
        .inherit_secrets([selected, route_value]);
        Ok(NodeResult {
            outputs: Outputs::from([("result".into(), value)]),
            ..Default::default()
        })
    }
}

fn validate_routes(routes: &[String]) -> Result<(), RouterBuildError> {
    let mut names = BTreeSet::new();
    for route in routes {
        ensure!(
            !route.trim().is_empty() && route != "else" && names.insert(route),
            RouteSnafu { route }
        );
    }
    Ok(())
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let Config {
        routes,
        mode,
        flow_field,
    } = mf_runtime::deserialize_config(config)?;
    match (routes, mode, flow_field) {
        (Some(routes), None, None) => {
            ensure!(!routes.is_empty(), EmptySnafu);
            validate_routes(&routes)?;
            let node = Router { routes };
            let metadata = NodeMetadata {
                exclusive_outputs: node
                    .ports()
                    .outputs
                    .iter()
                    .map(|port| port.name.to_string())
                    .collect(),
                ..Default::default()
            };
            PreparedNode::new(node, metadata)
        }
        (None, Some(Mode::Execute), flow_field) => {
            if let Some(field) = &flow_field {
                ensure!(!field.trim().is_empty(), FlowFieldSnafu);
            }
            PreparedNode::new(
                ExecuteRouter { flow_field },
                NodeMetadata {
                    // Dynamic callees' privacy contracts are not known when snapshots
                    // are taken; match workflow.call's conservative argument policy.
                    input_bindings: BTreeMap::from([(
                        "arguments".into(),
                        mf_runtime::InputBinding {
                            secret: true,
                            ..Default::default()
                        },
                    )]),
                    ..Default::default()
                },
            )
        }
        _ => ModeSnafu.fail().map_err(NodeBuildError::from),
    }
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        factory: NodeFactory::Plain(factory),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{FlowValue, GraphRelations, PreparedSubgraph};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn body() -> PreparedSubgraph {
        PreparedSubgraph::new(vec![], vec![], |_| Ok(Default::default()))
    }

    fn handle(body: PreparedSubgraph) -> ValueRef {
        FlowValue::new(body).unwrap().into()
    }

    fn flows(body: PreparedSubgraph) -> ValueRef {
        ValueRef::object([("tool".into(), handle(body))])
    }

    fn executable_with(config: Value) -> PreparedNode {
        mf_runtime::NodeRegistry::from_inventory()
            .unwrap()
            .get(KIND)
            .unwrap()
            .instantiate(config)
            .unwrap()
    }

    fn executable() -> PreparedNode {
        executable_with(json!({"mode": "execute"}))
    }

    fn field_executable(field: &str) -> PreparedNode {
        executable_with(json!({"mode": "execute", "flow_field": field}))
    }

    fn execute_with(
        node: &PreparedNode,
        route: ValueRef,
        flows: ValueRef,
        arguments: ValueRef,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        node.execution.as_task_node().unwrap().execute(
            Inputs::from([
                ("route".into(), route),
                ("flows".into(), flows),
                ("arguments".into(), arguments),
            ]),
            context,
        )
    }

    fn execute(flows: ValueRef) -> Result<NodeResult, NodeExecutionError> {
        execute_with(
            &executable(),
            "tool".into(),
            flows,
            json!({}).into(),
            &mut ExecutionContext::default(),
        )
    }

    fn business_error(error: &NodeBuildError) -> &RouterBuildError {
        let NodeBuildError::FactoryFailed { source } = error else {
            panic!("expected owning-node business error, got {error}");
        };
        source.downcast_ref().unwrap()
    }

    fn execution_error(error: &NodeExecutionError) -> &RouterExecutionError {
        let NodeExecutionError::PluginFailed { source } = error else {
            panic!("expected owning-node execution error, got {error}");
        };
        source.downcast_ref().unwrap()
    }

    fn has_source<T: std::error::Error + 'static>(error: &dyn std::error::Error) -> bool {
        let mut source = error.source();
        while let Some(cause) = source {
            if cause.is::<T>() {
                return true;
            }
            source = cause.source();
        }
        false
    }

    fn schema() -> mf_runtime::WorkflowInputSchema {
        serde_json::from_value(json!({
            "inputs": {"params": {"value": {"type": "any", "required": true}}},
            "stdin": {}
        }))
        .unwrap()
    }

    #[test]
    fn gate_selects_one_exclusive_output_and_skips_the_rest() {
        let node = factory(json!({"routes": ["first", "second"]})).unwrap();
        assert_eq!(node.metadata.exclusive_outputs, ["first", "second", "else"]);
        assert_eq!(
            node.metadata.ports.inputs,
            [PortSpec::new("route", ValueType::String, true)]
        );
        for (requested, selected) in [
            ("first", "first"),
            ("second", "second"),
            ("missing", "else"),
        ] {
            let result = node
                .execution
                .as_task_node()
                .unwrap()
                .execute(
                    Inputs::from([("route".into(), requested.into())]),
                    &mut ExecutionContext::default(),
                )
                .unwrap();
            assert_eq!(result.outputs.len(), 1);
            assert_eq!(result.outputs[selected], json!(true));
            assert_eq!(result.skipped.len(), 2);
            assert!(!result.skipped.contains(selected));
        }
        let result = node
            .execution
            .as_task_node()
            .unwrap()
            .execute(
                Inputs::from([("route".into(), ValueRef::from("first").into_secret())]),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        assert!(result.outputs["first"].is_secret());
    }

    #[test]
    fn config_is_strict_and_has_no_static_flow_parameters() {
        for config in [
            json!({"routes": []}),
            json!({"routes": [""]}),
            json!({"routes": [" "]}),
            json!({"routes": ["else"]}),
            json!({"routes": ["a", "a"]}),
        ] {
            assert!(matches!(
                business_error(&factory(config).err().unwrap()),
                RouterBuildError::Empty | RouterBuildError::Route { .. }
            ));
        }
        for config in [json!({}), json!({"routes": ["a"], "mode": "execute"})] {
            assert!(matches!(
                business_error(&factory(config).err().unwrap()),
                RouterBuildError::Mode
            ));
        }
        for config in [
            json!({"subgraphs": {}}),
            json!({"mode": "gate"}),
            json!({"mode": "execute", "unknown": true}),
            json!({"routes": false}),
            json!({"routes": [1]}),
        ] {
            assert!(matches!(
                factory(config),
                Err(NodeBuildError::InvalidConfiguration { .. })
            ));
        }
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let registration = registry.get(KIND).unwrap();
        assert!(matches!(registration.factory, NodeFactory::Plain(_)));
        assert!(
            registration
                .flow_parameters(&json!({"mode": "execute"}))
                .is_empty()
        );
    }

    #[test]
    fn flow_field_config_is_strict_and_execute_only() {
        for config in [
            json!({"routes": ["a"], "flow_field": "run"}),
            json!({"routes": ["a"], "mode": "execute", "flow_field": "run"}),
            json!({"flow_field": "run"}),
        ] {
            assert!(matches!(
                business_error(&factory(config).err().unwrap()),
                RouterBuildError::Mode
            ));
        }
        for field in ["", " ", "\t\n"] {
            assert!(matches!(
                business_error(
                    &factory(json!({"mode": "execute", "flow_field": field}))
                        .err()
                        .unwrap()
                ),
                RouterBuildError::FlowField
            ));
        }
        for field in [json!(1), json!(true), json!([])] {
            assert!(matches!(
                factory(json!({"mode": "execute", "flow_field": field})),
                Err(NodeBuildError::InvalidConfiguration { .. })
            ));
        }
    }

    #[test]
    fn execute_ports_and_argument_privacy_are_declared_without_static_candidates() {
        let node = executable();
        assert_eq!(
            node.metadata.ports.inputs,
            [
                PortSpec::new("route", ValueType::String, true),
                PortSpec::new("flows", ValueType::Map(Box::new(ValueType::Flow)), true),
                PortSpec::new("arguments", ValueType::Object, true),
            ]
        );
        assert_eq!(
            node.metadata.ports.outputs,
            [PortSpec::new("result", ValueType::Object, true)]
        );
        assert!(node.metadata.stream_outputs.is_empty());
        assert!(node.metadata.exclusive_outputs.is_empty());
        assert_eq!(node.metadata.input_bindings.len(), 1);
        assert!(node.metadata.input_bindings["arguments"].secret);
        assert!(matches!(node.execution, mf_runtime::NodeExecution::Task(_)));
    }

    #[test]
    fn flow_field_declares_object_records_without_changing_other_ports() {
        let node = field_executable("run");
        assert_eq!(
            node.metadata.ports.inputs,
            [
                PortSpec::new("route", ValueType::String, true),
                PortSpec::new("flows", ValueType::Map(Box::new(ValueType::Object)), true),
                PortSpec::new("arguments", ValueType::Object, true),
            ]
        );
        assert_eq!(
            node.metadata.ports.outputs,
            [PortSpec::new("result", ValueType::Object, true)]
        );
        assert_eq!(node.metadata.input_bindings.len(), 1);
        assert!(node.metadata.input_bindings["arguments"].secret);
    }

    #[test]
    fn native_handles_keep_identity_and_only_the_selected_handle_executes() {
        let calls = Arc::new(AtomicUsize::new(0));
        let returned = handle(body());
        let mut entries = BTreeMap::new();
        for name in ["a/b~", "else", " "] {
            let calls = calls.clone();
            let returned = returned.clone();
            let flow = handle(PreparedSubgraph::new(
                vec![],
                vec![PortSpec::new("handle", ValueType::Flow, true)],
                move |context| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(context.invocation_path().len(), 1);
                    assert!(context.invocation_path()[0].is_call());
                    assert_eq!(context.invocation_path()[0].node_id(), "owner");
                    Ok(Outputs::from([("handle".into(), returned.clone())]))
                },
            ));
            entries.insert(name.into(), flow);
        }
        let candidates = ValueRef::object(entries);
        let original = candidates["a/b~"].clone();
        assert_eq!(candidates["a/b~"].as_flow(), original.as_flow());
        assert!(candidates["a/b~"].ptr_eq(&original));
        let node = executable();
        let mut context = ExecutionContext::default();
        for (index, name) in ["a/b~", "else", " "].into_iter().enumerate() {
            let result = context
                .with_node("owner".into(), |context| {
                    execute_with(
                        &node,
                        name.into(),
                        candidates.clone(),
                        json!({}).into(),
                        context,
                    )
                })
                .unwrap();
            assert!(result.outputs["result"]["outputs"]["handle"].ptr_eq(&returned));
            assert_eq!(calls.load(Ordering::SeqCst), index + 1);
        }
        let error = execute_with(
            &node,
            "missing".into(),
            candidates,
            json!({}).into(),
            &mut context,
        )
        .unwrap_err();
        assert!(matches!(
            execution_error(&error),
            RouterExecutionError::UnknownRoute { route, .. } if route == "missing"
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(matches!(
            execution_error(&execute(json!({"tool": {}}).into()).unwrap_err()),
            RouterExecutionError::InvalidFlow { .. }
        ));
    }

    #[test]
    fn flow_field_preserves_native_identity_and_executes_only_the_selected_field() {
        let calls = Arc::new(AtomicUsize::new(0));
        let returned = handle(body());
        let mut entries = BTreeMap::new();
        for name in ["first", "second"] {
            let calls = calls.clone();
            let returned = returned.clone();
            let selected = handle(PreparedSubgraph::new(
                vec![],
                vec![PortSpec::new("handle", ValueType::Flow, true)],
                move |context| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(context.invocation_path().len(), 1);
                    assert_eq!(context.invocation_path()[0].node_id(), "owner");
                    Ok(Outputs::from([("handle".into(), returned.clone())]))
                },
            ));
            entries.insert(
                name.into(),
                ValueRef::object([
                    ("run".into(), selected),
                    ("description".into(), "metadata".into()),
                ]),
            );
        }
        let records = ValueRef::object(entries);
        let original = records["first"]["run"].clone();
        assert_eq!(records["first"]["run"].as_flow(), original.as_flow());
        assert!(records["first"]["run"].ptr_eq(&original));
        let node = field_executable("run");
        let mut context = ExecutionContext::default();
        for (index, name) in ["first", "second"].into_iter().enumerate() {
            let result = context
                .with_node("owner".into(), |context| {
                    execute_with(
                        &node,
                        name.into(),
                        records.clone(),
                        json!({}).into(),
                        context,
                    )
                })
                .unwrap();
            assert!(result.outputs["result"]["outputs"]["handle"].ptr_eq(&returned));
            assert_eq!(calls.load(Ordering::SeqCst), index + 1);
        }
        let error = execute_with(
            &node,
            "missing".into(),
            records,
            json!({}).into(),
            &mut context,
        )
        .unwrap_err();
        assert!(matches!(
            execution_error(&error),
            RouterExecutionError::UnknownRoute { route, .. } if route == "missing"
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn flow_field_missing_and_non_flow_values_are_invalid_flows() {
        let node = field_executable("run");
        for record in [
            json!({}).into(),
            json!({"run": {}}).into(),
            json!({"run": "not native"}).into(),
        ] {
            let records = ValueRef::object([("selected".into(), record)]);
            let error = execute_with(
                &node,
                "selected".into(),
                records,
                json!({}).into(),
                &mut Default::default(),
            )
            .unwrap_err();
            assert!(matches!(
                execution_error(&error),
                RouterExecutionError::InvalidFlow { route, .. } if route == "selected"
            ));
        }
    }

    #[test]
    fn arguments_are_qualified_native_values_and_each_call_is_isolated() {
        let outer_node = mf_runtime::FlowNode {
            definition_id: "outer".into(),
            node: (),
            metadata: NodePorts {
                inputs: vec![],
                outputs: vec![PortSpec::new("value", ValueType::Int64, true)],
            }
            .into(),
            output_keys: Default::default(),
        };
        let mut context = ExecutionContext::for_message(
            &outer_node,
            Outputs::from([("value".into(), 99.into())]).into(),
        )
        .unwrap();
        context.set_workflow_arguments(WorkflowArguments(Inputs::from([(
            "ambient.secret".into(),
            "not captured".into(),
        )])));
        let candidates = flows(
            PreparedSubgraph::new(
                vec![],
                vec![PortSpec::new("value", ValueType::Any, true)],
                |child| {
                    assert!(child.output("outer.value").is_err());
                    assert!(child.scope_path().is_empty());
                    assert!(child.current_node().is_none());
                    assert_eq!(child.workflow_arguments().0.len(), 1);
                    Ok(Outputs::from([(
                        "value".into(),
                        child.workflow_arguments().0["params.value"].clone(),
                    )]))
                },
            )
            .with_input_schema(schema()),
        );
        let node = executable();
        for argument in [ValueRef::from(1), handle(body()), ValueRef::from(2)] {
            let result = execute_with(
                &node,
                "tool".into(),
                candidates.clone(),
                ValueRef::object([("params.value".into(), argument.clone())]),
                &mut context,
            )
            .unwrap();
            assert!(result.outputs["result"]["outputs"]["value"].ptr_eq(&argument));
            assert!(context.invocation_path().is_empty());
            assert_eq!(context.workflow_arguments().0.len(), 1);
            assert!(
                context
                    .workflow_arguments()
                    .0
                    .contains_key("ambient.secret")
            );
            assert_eq!(
                context
                    .select_output("value", "outer", "value", false)
                    .unwrap(),
                Some(99.into())
            );
        }
    }

    #[test]
    fn selected_flow_validates_arguments_before_body_effects() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executions = calls.clone();
        let candidates = flows(
            PreparedSubgraph::new(vec![], vec![], move |_| {
                executions.fetch_add(1, Ordering::SeqCst);
                Ok(Default::default())
            })
            .with_input_schema(schema()),
        );
        let result = execute(candidates).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // Invalid caller arguments stay a business result and never reach the body.
        assert!(
            result.outputs["result"]["error"]
                .as_str()
                .is_some_and(|error| error.contains("required")),
            "{}",
            result.outputs["result"]
        );
        assert!(result.outputs["result"].is_secret());
    }

    #[test]
    fn finite_streams_are_drained_without_exposing_emissions() {
        let candidates = flows(PreparedSubgraph::streaming(
            vec![],
            vec![PortSpec::new("answer", ValueType::String, true)],
            vec![PortSpec::new("chunk", ValueType::String, true)],
            |_, sink| {
                assert!(!sink.is_connected());
                sink.send(NodeResult {
                    outputs: Outputs::from([("chunk".into(), "incremental".into())]),
                    ..Default::default()
                })
                .unwrap();
                Ok(Outputs::from([("answer".into(), "terminal".into())]))
            },
        ));
        assert_eq!(
            execute(candidates).unwrap().outputs["result"],
            json!({"outputs": {"answer": "terminal"}})
        );
    }

    #[test]
    fn selection_privacy_does_not_blanket_taint_public_results_from_private_arguments() {
        let public_body = PreparedSubgraph::new(
            vec![],
            vec![PortSpec::new("answer", ValueType::String, true)],
            |_| Ok(Outputs::from([("answer".into(), "constant".into())])),
        );
        for secret_selected in [false, true] {
            for secret_route in [false, true] {
                for secret_arguments in [false, true] {
                    let selected = handle(public_body.clone());
                    let candidates = ValueRef::object([
                        (
                            "tool".into(),
                            if secret_selected {
                                selected.into_secret()
                            } else {
                                selected
                            },
                        ),
                        // An unrelated private candidate must not taint this result.
                        ("unused".into(), handle(body()).into_secret()),
                    ]);
                    let route = ValueRef::from("tool");
                    let arguments = ValueRef::from(json!({}));
                    let result = execute_with(
                        &executable(),
                        if secret_route {
                            route.into_secret()
                        } else {
                            route
                        },
                        candidates,
                        if secret_arguments {
                            arguments.into_secret()
                        } else {
                            arguments
                        },
                        &mut Default::default(),
                    )
                    .unwrap();
                    let result = &result.outputs["result"];
                    assert_eq!(result.is_secret(), secret_selected || secret_route);
                    assert_eq!(
                        result["outputs"]["answer"].is_secret(),
                        secret_selected || secret_route
                    );
                    assert_eq!(result["outputs"]["answer"], json!("constant"));
                }
            }
        }
        let result = execute(flows(PreparedSubgraph::new(
            vec![],
            vec![
                PortSpec::new("private", ValueType::String, true),
                PortSpec::new("public", ValueType::String, true),
            ],
            |_| {
                Ok(Outputs::from([
                    ("private".into(), ValueRef::from("hidden").into_secret()),
                    ("public".into(), "visible".into()),
                ]))
            },
        )))
        .unwrap();
        let result = &result.outputs["result"];
        assert!(!result.is_secret());
        assert!(result["outputs"]["private"].is_secret());
        assert!(!result["outputs"]["public"].is_secret());
        assert_eq!(
            serde_json::to_value(result.redacted()).unwrap(),
            json!({"outputs": {"private": "[REDACTED]", "public": "visible"}})
        );
    }

    #[test]
    fn flow_field_result_privacy_uses_only_the_selected_flow_and_route() {
        let public_body = PreparedSubgraph::new(
            vec![],
            vec![PortSpec::new("answer", ValueType::String, true)],
            |_| Ok(Outputs::from([("answer".into(), "constant".into())])),
        );
        let record = |selected| {
            ValueRef::object([
                ("run".into(), selected),
                (
                    "description".into(),
                    ValueRef::from("private metadata").into_secret(),
                ),
            ])
        };
        let records = ValueRef::object([
            ("selected".into(), record(handle(public_body.clone()))),
            (
                "unused".into(),
                record(handle(body()).into_secret()).into_secret(),
            ),
        ]);
        let node = field_executable("run");
        let result = execute_with(
            &node,
            "selected".into(),
            records,
            json!({}).into(),
            &mut Default::default(),
        )
        .unwrap();
        let result = &result.outputs["result"];
        assert!(!result.is_secret());
        assert!(!result["outputs"]["answer"].is_secret());
        assert_eq!(result["outputs"]["answer"], json!("constant"));

        for (records, route) in [
            (
                ValueRef::object([(
                    "selected".into(),
                    record(handle(public_body.clone()).into_secret()),
                )]),
                ValueRef::from("selected"),
            ),
            (
                ValueRef::object([(
                    "selected".into(),
                    record(handle(public_body.clone())).into_secret(),
                )]),
                ValueRef::from("selected"),
            ),
            (
                ValueRef::object([("selected".into(), record(handle(public_body)))]),
                ValueRef::from("selected").into_secret(),
            ),
        ] {
            let result = execute_with(
                &node,
                route,
                records,
                json!({}).into(),
                &mut Default::default(),
            )
            .unwrap();
            assert!(result.outputs["result"].is_secret());
            assert!(result.outputs["result"]["outputs"]["answer"].is_secret());
        }
    }

    #[test]
    fn arguments_are_redacted_in_snapshots_before_dynamic_callee_binding() {
        let mut schema = schema();
        schema
            .inputs
            .get_mut("params")
            .unwrap()
            .get_mut("value")
            .unwrap()
            .binding
            .secret = true;
        let candidates = flows(
            PreparedSubgraph::new(
                vec![],
                vec![PortSpec::new("value", ValueType::Any, true)],
                |child| {
                    let argument = child.workflow_arguments().0["params.value"].clone();
                    assert!(argument.is_secret());
                    Ok(Outputs::from([("value".into(), argument)]))
                },
            )
            .with_input_schema(schema)
            .with_origin(mf_runtime::FlowOrigin {
                workflow_id: mf_telemetry::identity::WorkflowId::from_definition(
                    &json!({"test": "router argument snapshots"}),
                    &[],
                )
                .unwrap(),
                path: vec![mf_runtime::ScopePathEntry::binding_key("source", "body")],
            }),
        );
        let source = mf_runtime::FlowNode {
            definition_id: "source".into(),
            node: (),
            metadata: NodePorts {
                inputs: vec![],
                outputs: vec![
                    PortSpec::new("route", ValueType::String, true),
                    PortSpec::new("flows", ValueType::Map(Box::new(ValueType::Flow)), true),
                    PortSpec::new("arguments", ValueType::Object, true),
                ],
            }
            .into(),
            output_keys: Default::default(),
        };
        let mut context = ExecutionContext::for_message(
            &source,
            Outputs::from([
                ("route".into(), "tool".into()),
                ("flows".into(), candidates),
                (
                    "arguments".into(),
                    json!({"params.value":"8675309-private"}).into(),
                ),
            ])
            .into(),
        )
        .unwrap();
        let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events = captured.clone();
        context.set_snapshot_recorder(mf_runtime::SnapshotRecorder::new(move |event| {
            events.lock().unwrap().push(event);
        }));
        let node = mf_runtime::FlowNode::new("router", executable())
            .into_task()
            .unwrap();
        let dependencies =
            ["route", "flows", "arguments"].map(|port| mf_runtime::ExecutionDependency {
                exclusive: false,
                input: Some(port),
                source_node: "source",
                source_output: port,
                source_key: format!("source.{port}").into(),
            });
        mf_runtime::execute_node_in_context(&node, &dependencies, &mut context).unwrap();
        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 2);
        for event in captured.iter() {
            assert_eq!(
                event.snapshot.inputs["arguments"].as_str(),
                Some("[REDACTED]")
            );
            assert_eq!(event.snapshot.inputs["route"].as_str(), Some("tool"));
        }
        assert_eq!(
            captured[1].snapshot.outputs["result"]["outputs"]["value"].as_str(),
            Some("[REDACTED]")
        );
        assert!(
            !serde_json::to_string(&*captured)
                .unwrap()
                .contains("8675309-private")
        );
    }

    #[test]
    fn secret_unknown_routes_are_redacted_in_errors_and_failed_snapshots() {
        let private_route = "private-missing-tool-selection";
        let source = mf_runtime::FlowNode {
            definition_id: "source".into(),
            node: (),
            metadata: NodePorts {
                inputs: vec![],
                outputs: executable().metadata.ports.inputs.clone(),
            }
            .into(),
            output_keys: Default::default(),
        };
        let mut context = ExecutionContext::for_message(
            &source,
            Outputs::from([
                ("route".into(), ValueRef::from(private_route).into_secret()),
                ("flows".into(), ValueRef::object([])),
                ("arguments".into(), json!({}).into()),
            ])
            .into(),
        )
        .unwrap();
        let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events = captured.clone();
        context.set_snapshot_recorder(mf_runtime::SnapshotRecorder::new(move |event| {
            events.lock().unwrap().push(event);
        }));
        let node = mf_runtime::FlowNode::new("router", executable())
            .into_task()
            .unwrap();
        let dependencies =
            ["route", "flows", "arguments"].map(|port| mf_runtime::ExecutionDependency {
                exclusive: false,
                input: Some(port),
                source_node: "source",
                source_output: port,
                source_key: format!("source.{port}").into(),
            });
        let error =
            mf_runtime::execute_node_in_context(&node, &dependencies, &mut context).unwrap_err();
        assert!(error.to_string().contains("[REDACTED]"));
        assert!(!error.to_string().contains(private_route));
        assert!(!format!("{error:?}").contains(private_route));
        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 2);
        for event in captured.iter() {
            assert_eq!(event.snapshot.inputs["route"].as_str(), Some("[REDACTED]"));
        }
        assert_eq!(
            captured[1].snapshot.outcome,
            mf_runtime::SnapshotOutcome::Failed
        );
        assert!(
            captured[1]
                .snapshot
                .error
                .as_ref()
                .unwrap()
                .contains("[REDACTED]")
        );
        assert!(
            !serde_json::to_string(&*captured)
                .unwrap()
                .contains(private_route)
        );
    }

    #[test]
    fn secret_route_diagnostics_preserve_invalid_flow_and_child_error_types() {
        let private_route = "private-selected-tool";
        for invalid in [false, true] {
            let selected = if invalid {
                json!({}).into()
            } else {
                handle(PreparedSubgraph::new(vec![], vec![], |_| {
                    Err(WorkflowRunError::Context {
                        definition_id: "child".into(),
                        message: "infrastructure failure".into(),
                    })
                }))
            };
            let error = execute_with(
                &executable(),
                ValueRef::from(private_route).into_secret(),
                ValueRef::object([(private_route.into(), selected)]),
                json!({}).into(),
                &mut Default::default(),
            )
            .unwrap_err();
            assert!(error.to_string().contains("[REDACTED]"));
            assert!(!error.to_string().contains(private_route));
            assert!(!format!("{error:?}").contains(private_route));
            if invalid {
                assert!(matches!(
                    execution_error(&error),
                    RouterExecutionError::InvalidFlow { .. }
                ));
            } else {
                assert!(
                    matches!(execution_error(&error), RouterExecutionError::Child {
                    source: WorkflowRunError::Context { message, .. }, ..
                } if message == "infrastructure failure")
                );
                assert!(has_source::<WorkflowRunError>(&error));
            }
        }
    }

    #[test]
    fn input_call_business_failures_recover_but_typed_infrastructure_errors_propagate() {
        assert_call_failure_recovery(false);
    }

    #[test]
    fn static_call_business_failures_recover_but_typed_infrastructure_errors_propagate() {
        assert_call_failure_recovery(true);
    }

    fn assert_call_failure_recovery(static_call: bool) {
        #[derive(mf_runtime::NodeValue)]
        struct InvalidOutput {
            value: f64,
        }
        for kind in [
            "business", "context", "stream", "worker", "contract", "encode", "plugin",
        ] {
            let inner = PreparedSubgraph::new(vec![], vec![], move |_| {
                let source = match kind {
                    "business" => mf_runtime::NodeExecutionFailedSnafu {
                        message: "nested business failure",
                    }
                    .build(),
                    "context" => {
                        return Err(WorkflowRunError::Context {
                            definition_id: "inner".into(),
                            message: "scope/resource failure".into(),
                        });
                    }
                    "stream" => mf_runtime::StreamExecutionSnafu {
                        message: "stream failure",
                    }
                    .build()
                    .into(),
                    "worker" => mf_runtime::WorkerPoolError {
                        worker: 0,
                        source: std::io::Error::other("worker failure"),
                    }
                    .into(),
                    "contract" => {
                        NodeExecutionError::from(Box::<dyn std::error::Error + Send + Sync>::from(
                            mf_telemetry::ContractError::Invalid {
                                message: "contract failure".into(),
                            },
                        ))
                    }
                    "encode" => {
                        return mf_runtime::encode_typed_result(mf_runtime::TypedNodeResult::from(
                            InvalidOutput { value: f64::NAN },
                        ))
                        .map(|result| result.outputs)
                        .context(mf_runtime::NodeExecutionSnafu {
                            definition_id: "inner",
                        });
                    }
                    _ => {
                        NodeExecutionError::from(Box::<dyn std::error::Error + Send + Sync>::from(
                            std::io::Error::other("unknown plugin failure"),
                        ))
                    }
                };
                Err(source).context(mf_runtime::NodeExecutionSnafu {
                    definition_id: "inner",
                })
            });
            let registration = mf_runtime::NodeRegistry::from_inventory()
                .unwrap()
                .get(mf_runtime::CALL_KIND)
                .unwrap();
            let (call, inputs) = if static_call {
                (
                    registration
                        .instantiate_subgraphs(
                            "static-call",
                            json!({"flow": "child", "body": {}}),
                            BTreeMap::from([("body".into(), inner)]),
                        )
                        .unwrap(),
                    Inputs::new(),
                )
            } else {
                (
                    registration
                        .instantiate(json!({"inputs": [], "outputs": []}))
                        .unwrap(),
                    Inputs::from([("flow".into(), handle(inner))]),
                )
            };
            let call = call.execution.into_task_node().unwrap();
            let candidates = flows(PreparedSubgraph::new(vec![], vec![], move |context| {
                call.execute(inputs.clone(), context)
                    .map(|result| result.outputs)
                    .context(mf_runtime::NodeExecutionSnafu {
                        definition_id: if static_call {
                            "static-call"
                        } else {
                            "call-input"
                        },
                    })
            }));
            let result = execute(candidates);
            if kind == "business" {
                let result = result.unwrap();
                assert!(result.outputs["result"].is_secret());
                assert!(
                    result.outputs["result"]["error"]
                        .as_str()
                        .unwrap()
                        .contains("nested business failure")
                );
                continue;
            }
            let error = result.unwrap_err();
            if static_call {
                assert!(has_source::<crate::call::CallFailure>(&error));
            } else {
                assert!(has_source::<crate::CallInputError>(&error));
            }
            match kind {
                "stream" => assert!(has_source::<mf_runtime::StreamError>(&error)),
                "worker" => assert!(has_source::<mf_runtime::WorkerPoolError>(&error)),
                "contract" => assert!(has_source::<mf_telemetry::ContractError>(&error)),
                "encode" => assert!(has_source::<Box<mf_runtime::OutputEncodeError>>(&error)),
                "plugin" => assert!(has_source::<std::io::Error>(&error)),
                _ => assert!(error.to_string().contains("scope/resource failure")),
            }
        }
    }

    #[test]
    fn unknown_plugin_wrappers_do_not_authorize_business_recovery() {
        #[derive(Debug, Snafu)]
        #[snafu(display("unknown wrapper: {source}"))]
        struct UnknownWrapper {
            source: WorkflowRunError,
        }
        let candidates = flows(PreparedSubgraph::new(vec![], vec![], |_| {
            let source = mf_runtime::NodeExecutionFailedSnafu {
                message: "business failure behind unknown wrapper",
            }
            .fail::<Outputs>()
            .context(mf_runtime::NodeExecutionSnafu {
                definition_id: "inner",
            })
            .unwrap_err();
            let source =
                Box::<dyn std::error::Error + Send + Sync>::from(UnknownWrapper { source });
            Err(NodeExecutionError::from(source)).context(mf_runtime::NodeExecutionSnafu {
                definition_id: "unknown",
            })
        }));
        let error = execute(candidates).unwrap_err();
        assert!(has_source::<UnknownWrapper>(&error));
    }

    #[test]
    fn business_failures_become_conservatively_secret_error_envelopes() {
        let candidates = flows(PreparedSubgraph::new(vec![], vec![], |_| {
            mf_runtime::NodeExecutionFailedSnafu {
                message: "domain failure with private data",
            }
            .fail()
            .context(mf_runtime::NodeExecutionSnafu {
                definition_id: "child",
            })
        }));
        let result = execute(candidates).unwrap();
        let result = &result.outputs["result"];
        assert!(result["error"].as_str().unwrap().contains("domain failure"));
        assert!(result.get("outputs").is_none());
        assert!(result.is_secret());
        assert!(result["error"].is_secret());
        assert_eq!(
            serde_json::to_value(result.redacted()).unwrap(),
            json!("[REDACTED]")
        );
    }

    #[test]
    fn cancellation_is_never_captured_even_after_successful_child_completion() {
        let calls = Arc::new(AtomicUsize::new(0));
        let child_calls = calls.clone();
        let candidates = flows(PreparedSubgraph::new(vec![], vec![], move |state| {
            child_calls.fetch_add(1, Ordering::SeqCst);
            state
                .cancellation()
                .cancel(mf_runtime::StreamError::InputFailure {
                    message: "cancelled".into(),
                });
            Ok(Default::default())
        }));
        assert!(execute(candidates).is_err());
        let mut context = ExecutionContext::default();
        context
            .cancellation()
            .cancel(mf_runtime::StreamError::InputFailure {
                message: "already cancelled".into(),
            });
        assert!(
            executable()
                .execution
                .as_task_node()
                .unwrap()
                .execute(Inputs::new(), &mut context)
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn invocation_constraints_are_deferred_and_unselected_candidates_are_not_rejected() {
        let mut inputs = mf_runtime::WorkflowInputSchema::default();
        inputs
            .stdin
            .insert("read".into(), mf_runtime::StdinRequirement::Always);
        let scoped = body().with_relations(GraphRelations {
            scope: Some(mf_runtime::ScopeRelations {
                input: "scope".into(),
                values: BTreeMap::new(),
                state_outputs: BTreeMap::new(),
                writes: BTreeMap::new(),
                exits: Default::default(),
            }),
            ..Default::default()
        });
        let event =
            PreparedSubgraph::from_stream_factory(vec![], Default::default(), vec![], || {
                panic!("unselected Event candidate must not execute")
            });
        for (child, selected) in [
            (body().with_input_schema(inputs), true),
            (scoped, true),
            (event, false),
        ] {
            let candidates = ValueRef::object([
                ("tool".into(), handle(body())),
                ("constrained".into(), handle(child)),
            ]);
            assert!(execute(candidates.clone()).is_ok());
            if selected {
                let outcome = execute_with(
                    &executable(),
                    "constrained".into(),
                    candidates,
                    json!({}).into(),
                    &mut Default::default(),
                );
                match outcome {
                    // Caller-data violations are business results, not infrastructure.
                    Ok(result) => assert!(result.outputs["result"].get("error").is_some()),
                    Err(error) => assert!(matches!(
                        execution_error(&error),
                        RouterExecutionError::Child {
                            source: WorkflowRunError::Context { .. },
                            ..
                        }
                    )),
                }
            }
        }
    }

    #[test]
    fn unknown_runtime_and_observation_errors_propagate_with_typed_sources() {
        let workflow =
            mf_telemetry::identity::WorkflowId::from_definition(&json!({"test":"router"}), &[])
                .unwrap();
        let other =
            mf_telemetry::identity::WorkflowId::from_definition(&json!({"test":"other"}), &[])
                .unwrap();
        for kind in ["context", "origin", "iteration", "contract", "plugin"] {
            let workflow = workflow.clone();
            let other = other.clone();
            let candidates = flows(PreparedSubgraph::new(vec![], vec![], move |_| {
                if kind == "context" {
                    return Err(WorkflowRunError::Context {
                        definition_id: "child".into(),
                        message: "scope/resource/budget failure".into(),
                    });
                }
                let source: Box<dyn std::error::Error + Send + Sync> = match kind {
                    "origin" => Box::new(mf_telemetry::ContractError::UnsupportedFlowOrigin {
                        owning_workflow: workflow.clone(),
                        origin_workflow: other.clone(),
                    }),
                    "iteration" => {
                        Box::new(mf_telemetry::ContractError::UnsupportedFlowIterationScope {
                            definition: mf_runtime::FlowOrigin {
                                workflow_id: workflow.clone(),
                                path: vec![],
                            },
                        })
                    }
                    "contract" => Box::new(mf_telemetry::ContractError::Invalid {
                        message: "contract failure".into(),
                    }),
                    _ => Box::new(std::io::Error::other("unknown plugin failure")),
                };
                Err(NodeExecutionError::from(source)).context(mf_runtime::NodeExecutionSnafu {
                    definition_id: "child",
                })
            }));
            let error = execute(candidates).unwrap_err();
            assert!(has_source::<WorkflowRunError>(&error));
            if ["origin", "iteration", "contract"].contains(&kind) {
                assert!(has_source::<mf_telemetry::ContractError>(&error));
            }
        }
    }

    #[test]
    fn direct_and_nested_output_encoding_failures_keep_the_source_chain() {
        #[derive(mf_runtime::NodeValue)]
        struct InvalidOutput {
            value: f64,
        }
        fn invalid() -> Result<Outputs, WorkflowRunError> {
            let result =
                mf_runtime::encode_typed_result(mf_runtime::TypedNodeResult::from(InvalidOutput {
                    value: f64::NAN,
                }))
                .context(mf_runtime::NodeExecutionSnafu {
                    definition_id: "invalid",
                })?;
            Ok(result.outputs)
        }
        for nested in [false, true] {
            let inner =
                FlowValue::new(PreparedSubgraph::new(vec![], vec![], |_| invalid())).unwrap();
            let candidates = flows(PreparedSubgraph::new(vec![], vec![], move |context| {
                if nested {
                    inner.invoke("nested", WorkflowArguments::default(), context)
                } else {
                    invalid()
                }
            }));
            let error = execute(candidates).unwrap_err();
            assert!(
                has_source::<Box<mf_runtime::OutputEncodeError>>(&error),
                "output encoding source was lost: {error}"
            );
        }
    }

    #[test]
    fn worker_and_all_stream_errors_propagate_with_typed_sources() {
        for kind in ["worker", "startup", "sink", "execution"] {
            let candidates = flows(PreparedSubgraph::new(vec![], vec![], move |_| {
                if kind == "worker" {
                    return Err(mf_runtime::WorkerPoolError {
                        worker: 0,
                        source: std::io::Error::other("worker failure"),
                    }
                    .into());
                }
                let source = match kind {
                    "startup" => mf_runtime::StreamError::ThreadSpawn {
                        thread: "child".into(),
                        source: Arc::new(std::io::Error::other("startup failure")),
                    },
                    "sink" => mf_runtime::StreamError::OutputWrite {
                        source: Arc::new(std::io::Error::other("sink failure")),
                    },
                    _ => mf_runtime::StreamExecutionSnafu {
                        message: "execution failure",
                    }
                    .build(),
                };
                Err(NodeExecutionError::from(source)).context(mf_runtime::NodeExecutionSnafu {
                    definition_id: "child",
                })
            }));
            let error = execute(candidates).unwrap_err();
            assert!(
                has_source::<mf_runtime::WorkerPoolError>(&error)
                    || has_source::<mf_runtime::StreamError>(&error)
                    || matches!(
                        execution_error(&error),
                        RouterExecutionError::Child {
                            source: WorkflowRunError::WorkerPool { .. },
                            ..
                        }
                    ),
                "typed {kind} source was lost: {error}"
            );
            assert!(error.to_string().contains(&format!("{kind} failure")));
        }
    }
}
