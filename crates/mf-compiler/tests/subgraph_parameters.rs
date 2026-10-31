extern crate mfn_code as _;
extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeMetadata, NodePortContract,
    NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, PreparedNode, PreparedSubgraph,
    PreparedSubgraphParameter, PreparedSubgraphs, TaskNode, ValueType,
};
use serde_json::{Value, json};
use snafu::ensure;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Apply {
    id: String,
    child: PreparedSubgraph,
}
impl NodePortContract for Apply {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("x", ValueType::Int64, true)],
            outputs: vec![PortSpec::new("value", ValueType::Any, true)],
        }
    }
}
impl TaskNode for Apply {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let arguments = self
            .child
            .bind_arguments(inputs)
            .map_err(Box::<dyn std::error::Error + Send + Sync>::from)?;
        let outputs = context.run_call(&self.id, arguments, &self.child.nodes, |state| {
            self.child.execute_in_context(state)
        })?;
        Ok(outputs.into())
    }
}
fn apply(
    id: &str,
    _: Value,
    _: Value,
    mut parameters: PreparedSubgraphs,
) -> Result<PreparedNode, NodeBuildError> {
    let child = mf_runtime::take_subgraph(&mut parameters, "child")?;
    ensure!(
        parameters.is_empty(),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "unknown Apply parameter"
        }
    );
    let graph = child.graph.clone();
    PreparedNode::new(
        Apply {
            id: id.into(),
            child,
        },
        NodeMetadata {
            subgraph: graph,
            ..Default::default()
        },
    )
}
inventory::submit! { NodeRegistration { kind: "test.subgraph_apply", factory: mf_runtime::NodeFactory::Subgraphs(apply) } }

static EFFECTS: AtomicUsize = AtomicUsize::new(0);
struct Effect;
impl NodePortContract for Effect {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::new("value", ValueType::Int64, true)],
        }
    }
}
impl TaskNode for Effect {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        EFFECTS.fetch_add(1, Ordering::SeqCst);
        Ok(Outputs::from([("value".into(), 7.into())]).into())
    }
}
fn effect(_: Value) -> Result<PreparedNode, NodeBuildError> {
    PreparedNode::new(Effect, NodeMetadata::default())
}
inventory::submit! { NodeRegistration { kind: "test.subgraph_effect", factory: mf_runtime::NodeFactory::Plain(effect) } }

struct Inspect(usize);
impl NodePortContract for Inspect {
    fn ports(&self) -> NodePorts {
        Effect.ports()
    }
}
impl TaskNode for Inspect {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        Ok(Outputs::from([("value".into(), (self.0 as u64).into())]).into())
    }
}
fn inspect(
    _: &str,
    _: Value,
    _: Value,
    mut parameters: PreparedSubgraphs,
) -> Result<PreparedNode, NodeBuildError> {
    let Some(PreparedSubgraphParameter::Interface(interface)) = parameters.remove("child") else {
        return mf_runtime::NodeInvalidSubgraphSnafu {
            message: "Inspect requires an interface",
        }
        .fail();
    };
    PreparedNode::new(Inspect(interface.outputs.len()), NodeMetadata::default())
}
inventory::submit! { NodeRegistration { kind: "test.subgraph_inspect", factory: mf_runtime::NodeFactory::Subgraphs(inspect) } }

fn code_node() -> Value {
    json!({"id":"operation","kind":"builtin.code","config":{"language":"cel","inputs":{"x":"int"},"code":{"value":"x + 1"}}})
}
fn code_flow() -> Value {
    json!({"version":"2026-10-31","dependencies":{},"nodes":[code_node()],"outputs":[{"name":"value","node":"operation","port":"value"}]})
}
fn node_source() -> Value {
    json!({"node":code_node()})
}
fn flow_source() -> Value {
    json!({"source":{"definition":code_flow()},"inputs":{"x":"operation.x"}})
}
fn decode(value: &Value) -> WorkflowDefinition {
    serde_json::from_value(value.clone()).unwrap()
}
fn execute(value: &Value) -> Value {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&decode(value), &registry).unwrap();
    serde_json::to_value(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap(),
    )
    .unwrap()
}
fn node_parent(source: Value) -> Value {
    json!({"version":"2026-10-31","dependencies":{},"nodes":[
        {"id":"seed","kind":"builtin.constant","config":{"value":41}},
        {"id":"apply","kind":"test.subgraph_apply","subgraphs":{"child":source},"inputs":{"x":{"from":{"node":"seed","port":"value"}}}}
    ],"outputs":[{"name":"value","node":"apply","port":"value"}]})
}
fn template() -> Value {
    json!({"version":"2026-10-31","dependencies":{},"subgraphs":{"child":{"parameter":"single"}},"nodes":[
        {"id":"seed","kind":"builtin.constant","config":{"value":41}},
        {"id":"invoke","kind":"workflow.call","config":{"flow":"child"},"inputs":{"x":{"from":{"node":"seed","port":"value"}}}}
    ],"outputs":[{"name":"value","node":"invoke","port":"value"}]})
}
fn flow_parent(source: Value) -> Value {
    json!({"version":"2026-10-31","dependencies":{},"subgraphs":{"parent":{"definition":template()}},"nodes":[
        {"id":"parent","kind":"workflow.call","config":{"flow":"parent"},"subgraphs":{"child":source}}
    ],"outputs":[{"name":"value","node":"parent","port":"value"}]})
}

#[test]
fn node_and_flow_arguments_work_with_node_and_flow_parents() {
    for source in [node_source(), flow_source()] {
        assert_eq!(execute(&node_parent(source.clone())), json!({"value":42}));
        assert_eq!(execute(&flow_parent(source)), json!({"value":42}));
    }
}

#[test]
fn nested_flows_forward_named_arguments_without_executing_the_registration() {
    let inner = template();
    let outer = json!({"version":"2026-10-31","dependencies":{},"subgraphs":{"handler":{"parameter":"single"},"inner":{"definition":inner}},"nodes":[
        {"id":"inner","kind":"workflow.call","config":{"flow":"inner"},"subgraphs":{"child":{"reference":"handler"}}}
    ],"outputs":[{"name":"value","node":"inner","port":"value"}]});
    let value = json!({"version":"2026-10-31","dependencies":{},"subgraphs":{"outer":{"definition":outer}},"nodes":[
        {"id":"outer","kind":"workflow.call","config":{"flow":"outer"},"subgraphs":{"handler":node_source()}}
    ],"outputs":[{"name":"value","node":"outer","port":"value"}]});
    assert_eq!(execute(&value), json!({"value":42}));
}

#[test]
fn reused_arguments_keep_call_inputs_isolated_and_preserve_secret_values() {
    let mut value = node_parent(json!({"reference":"handler"}));
    value["subgraphs"] = json!({"handler":node_source()});
    value["nodes"].as_array_mut().unwrap().extend([
        json!({"id":"other_seed","kind":"builtin.constant","config":{"value":10}}),
        json!({"id":"other","kind":"test.subgraph_apply","subgraphs":{"child":{"reference":"handler"}},"inputs":{"x":{"from":{"node":"other_seed","port":"value"}}}}),
    ]);
    value["outputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"other","node":"other","port":"value"}));
    assert_eq!(execute(&value), json!({"value":42,"other":11}));

    let mut value = node_parent(node_source());
    value["nodes"].as_array_mut().unwrap().remove(0);
    value["nodes"][0].as_object_mut().unwrap().remove("inputs");
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&decode(&value), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let mut context = ExecutionContext::default();
    context.set_workflow_arguments(mf_runtime::WorkflowArguments(Inputs::from([(
        "apply.x".into(),
        mf_runtime::ValueRef::from(41).into_secret(),
    )])));
    let outputs = flow.execute_in_context(&mut context).unwrap();
    assert!(outputs["value"].is_secret());
}

#[test]
fn interface_projection_exposes_contracts_without_running_the_child() {
    EFFECTS.store(0, Ordering::SeqCst);
    let value = json!({"version":"2026-10-31","dependencies":{},"nodes":[
        {"id":"inspect","kind":"test.subgraph_inspect","subgraphs":{"child":{"source":{"node":{"id":"effect","kind":"test.subgraph_effect"}},"readonly":true}}}
    ],"outputs":[{"name":"value","node":"inspect","port":"value"}]});
    assert_eq!(execute(&value), json!({"value":1}));
    assert_eq!(EFFECTS.load(Ordering::SeqCst), 0);
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&decode(&value), &registry).unwrap();
    assert!(
        plan.manifest(&registry)
            .unwrap()
            .description()
            .unwrap()
            .loop_bodies
            .is_empty()
    );
    let callable = node_parent(json!({"source":node_source(),"readonly":true}));
    assert!(compile_definition(&decode(&callable), &registry).is_err());
}

#[test]
fn missing_unknown_mismatched_and_invalid_argument_interfaces_fail_during_preparation() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut missing = flow_parent(node_source());
    missing["nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("subgraphs");
    let mut unknown = flow_parent(node_source());
    unknown["nodes"][0]["subgraphs"] = json!({"unknown":node_source()});
    let mismatched = flow_parent(json!({"collection":{"one":node_source()}}));
    let undeclared = node_parent(json!({"reference":"missing"}));
    for value in [missing, unknown, mismatched, undeclared] {
        assert!(compile_definition(&decode(&value), &registry).is_err());
    }
    for projection in [
        json!({}),
        json!({"x":"missing"}),
        json!({"x":"x","other":"x"}),
    ] {
        let value = node_parent(json!({"source":node_source(),"inputs":projection}));
        assert!(compile_definition(&decode(&value), &registry).is_err());
    }
}

#[test]
fn declared_default_subgraphs_are_used_when_the_caller_omits_the_argument() {
    let mut parent = template();
    parent["subgraphs"]["child"]["default"] = node_source();
    let value = json!({"version":"2026-10-31","dependencies":{},"subgraphs":{"parent":{"definition":parent}},"nodes":[
        {"id":"parent","kind":"workflow.call","config":{"flow":"parent"}}
    ],"outputs":[{"name":"value","node":"parent","port":"value"}]});
    assert_eq!(execute(&value), json!({"value":42}));
}

#[test]
fn collection_defaults_resolve_lexical_sources_before_cardinality_validation() {
    let value = json!({"version":"2026-10-31","dependencies":{},"subgraphs":{
        "handlers":{"parameter":"collection","default":{"reference":"fallback"}},
        "fallback":{"collection":{"increment":node_source()}}
    },"nodes":[
        {"id":"selector","kind":"builtin.constant","config":{"value":"increment"}},
        {"id":"arguments","kind":"builtin.constant","config":{"value":{"x":41}}},
        {"id":"dispatch","kind":"builtin.dispatch","subgraphs":{"branches":{"reference":"handlers"}},"inputs":{"selector":{"from":{"node":"selector","port":"value"}},"arguments":{"from":{"node":"arguments","port":"value"}}}}
    ],"outputs":[{"name":"result","node":"dispatch","port":"result"}]});
    assert_eq!(execute(&value), json!({"result":{"value":42}}));
}

#[test]
fn graph_parameters_reuse_registered_flow_executors_without_leaf_providers() {
    let body_definition = decode(
        &json!({"version":"2026-10-31","dependencies":{},"nodes":[{"id":"operation","kind":"host.only"}],"outputs":[{"name":"value","node":"operation","port":"value"}]}),
    );
    let mut schema = mf_runtime::WorkflowInputSchema::default();
    schema.inputs.insert(
        "operation".into(),
        std::collections::BTreeMap::from([(
            "x".into(),
            mf_runtime::WorkflowInput {
                value_type: ValueType::Int64,
                required: true,
                binding: Default::default(),
                schema: Some(mf_runtime::ValueSchema::Type(ValueType::Int64)),
            },
        )]),
    );
    let body = PreparedSubgraph::new(
        vec![],
        vec![PortSpec::new("value", ValueType::Int64, true)],
        |context| {
            let x = context.workflow_arguments().0["operation.x"]
                .as_i64()
                .unwrap();
            Ok(Outputs::from([("value".into(), (x + 1).into())]))
        },
    )
    .with_input_schema(schema);
    for projected in [false, true] {
        let mut body = body.clone();
        if projected {
            let graph = mf_runtime::WorkflowGraph {
                inputs: mf_runtime::WorkflowGraph::describe_inputs(&body.inputs),
                ..Default::default()
            };
            body = body
                .with_graph(std::sync::Arc::new(graph))
                .with_input_aliases(std::collections::BTreeMap::from([(
                    "published".into(),
                    "operation.x".into(),
                )]));
        }
        let mut registry = NodeRegistry::from_inventory().unwrap();
        registry
            .register_flow("child", body_definition.clone(), body)
            .unwrap();
        let source = json!({"source":{"definition":body_definition},"inputs":{"x":if projected {"published"} else {"operation.x"}}});
        let plan = compile_definition(&decode(&node_parent(source)), &registry).unwrap();
        assert_eq!(
            instantiate_compiled(&plan, &registry)
                .unwrap()
                .execute()
                .unwrap()["value"],
            json!(42)
        );
        plan.manifest(&registry).unwrap().validate().unwrap();
    }
}

#[test]
fn readonly_views_can_be_forwarded_and_projected_without_regaining_execution() {
    let parent = json!({"version":"2026-10-31","dependencies":{},"subgraphs":{"handler":{"parameter":"single"}},"nodes":[
        {"id":"inspect","kind":"test.subgraph_inspect","subgraphs":{"child":{"source":{"reference":"handler"},"readonly":true}}}
    ],"outputs":[{"name":"value","node":"inspect","port":"value"}]});
    let value = json!({"version":"2026-10-31","dependencies":{},"subgraphs":{"parent":{"definition":parent}},"nodes":[
        {"id":"parent","kind":"workflow.call","config":{"flow":"parent"},"subgraphs":{"handler":{"source":node_source(),"inputs":{"renamed":"x"},"readonly":true}}}
    ],"outputs":[{"name":"value","node":"parent","port":"value"}]});
    assert_eq!(execute(&value), json!({"value":1}));
}

#[test]
fn readonly_views_cannot_be_called() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let value = flow_parent(json!({"source":node_source(),"readonly":true}));
    let error = compile_definition(&decode(&value), &registry)
        .unwrap_err()
        .to_string();
    assert!(error.contains("callable subgraph"), "{error}");
}

#[test]
fn legacy_callers_inherit_generic_subgraph_description_capabilities() {
    let value = json!({"version":"2026-10-28","dependencies":{},"nodes":[
        {"id":"outer","kind":"workflow.call","config":{"flow":"child","definition":node_parent(node_source())}}
    ],"outputs":[{"name":"value","node":"outer","port":"value"}]});
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&decode(&value), &registry).unwrap();
    let description = plan.manifest(&registry).unwrap().description().unwrap();
    assert_eq!(
        description.version,
        mf_telemetry::description::WorkflowDescriptionVersion::V2026_10_31
    );
    description
        .validate_scope_path(&[
            mf_telemetry::event::ScopePathEntry::Call {
                call_id: "outer".into(),
            },
            mf_telemetry::event::ScopePathEntry::Call {
                call_id: "apply".into(),
            },
        ])
        .unwrap();
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()["value"],
        json!(42)
    );
}

#[test]
fn loop_nodes_keep_native_body_bindings_when_passed_as_arguments() {
    let node = json!({"id":"repeat","kind":"workflow.loop","loop":{"max_iterations":2,"variables":[{"name":"value","type":"int"}],"body":{"nodes":[
        {"id":"increment","kind":"builtin.code","config":{"language":"cel","inputs":{"value":"int"},"code":{"value":"value + 1"}},"inputs":{"value":{"from":{"node":"%loop","port":"value"}}}},
        {"id":"save","kind":"workflow.loop_assign","config":{"variable":"value"},"inputs":{"value":{"from":{"node":"increment","port":"value"}}}}
    ]}}});
    let source = json!({"source":{"node":node},"inputs":{"x":"value"}});
    assert_eq!(execute(&node_parent(source.clone())), json!({"value":43}));
    assert_eq!(execute(&flow_parent(source)), json!({"value":43}));
}

#[test]
fn iteration_nodes_keep_native_item_bindings_when_passed_to_flows() {
    let callback = json!({"node":{"id":"each","kind":"builtin.iteration","config":{"body":{"nodes":[
        {"id":"increment","kind":"builtin.code","config":{"language":"cel","inputs":{"value":"int"},"code":{"value":"value + 1"}},"inputs":{"value":{"from":{"node":"%iteration","port":"item"}}}}
    ],"result":{"node":"increment","port":"value"}}}}});
    let parent = json!({"version":"2026-10-31","dependencies":{},"subgraphs":{"handler":{"parameter":"single"}},"nodes":[
        {"id":"items","kind":"builtin.constant","config":{"value":[1,2]}},
        {"id":"invoke","kind":"workflow.call","config":{"flow":"handler"},"inputs":{"items":{"from":{"node":"items","port":"value"}}}}
    ],"outputs":[{"name":"results","node":"invoke","port":"results"}]});
    let value = json!({"version":"2026-10-31","dependencies":{},"subgraphs":{"parent":{"definition":parent}},"nodes":[
        {"id":"parent","kind":"workflow.call","config":{"flow":"parent"},"subgraphs":{"handler":callback}}
    ],"outputs":[{"name":"results","node":"parent","port":"results"}]});
    assert_eq!(execute(&value), json!({"results":[2,3]}));
}

#[test]
fn invalid_reference_graphs_and_capability_upgrades_have_contextual_errors() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let invalid = [
        (json!({"collection":{" ":node_source()}}), "nonblank"),
        (
            json!({"collection":{"reserved:name":node_source()}}),
            "colons",
        ),
        (
            json!({"collection":{"nested":{"collection":{}}}}),
            "individual callable",
        ),
        (
            json!({"collection":{"readonly":{"source":node_source(),"readonly":true}}}),
            "individual callable",
        ),
        (
            json!({"source":{"collection":{}},"inputs":{}}),
            "requires one subgraph",
        ),
        (
            json!({"source":{"source":{"collection":{}},"readonly":true},"inputs":{}}),
            "requires one subgraph interface",
        ),
        (json!({"parameter":"single"}), "unbound subgraph parameter"),
        (json!({}), "exactly one"),
        (
            json!({"node":code_node(),"definition":code_flow()}),
            "exactly one",
        ),
        (
            json!({"source":node_source(),"inputs":{"":"x"}}),
            "must not be empty",
        ),
    ];
    for (source, expected) in invalid {
        let error = compile_definition(&decode(&node_parent(source)), &registry)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
    }
    let mut cyclic = node_parent(json!({"reference":"first"}));
    cyclic["subgraphs"] = json!({"first":{"reference":"second"},"second":{"reference":"first"}});
    let error = compile_definition(&decode(&cyclic), &registry)
        .unwrap_err()
        .to_string();
    assert!(error.contains("exceeds sixteen"), "{error}");

    for source in [
        json!({"source":node_source(),"readonly":true}),
        json!({"collection":{"one":node_source()}}),
    ] {
        let mut value = flow_parent(source);
        value["subgraphs"]["parent"]["definition"]["nodes"][1]["subgraphs"] =
            json!({"child":node_source()});
        value["subgraphs"]["parent"]["definition"]["subgraphs"]["child"]["parameter"] =
            if value["nodes"][0]["subgraphs"]["child"]
                .get("collection")
                .is_some()
            {
                "collection".into()
            } else {
                "single".into()
            };
        assert!(compile_definition(&decode(&value), &registry).is_err());
    }
    let native = json!({"version":"2026-10-31","dependencies":{},"nodes":[{"id":"repeat","kind":"workflow.loop","subgraphs":{"body":node_source()},"loop":{"max_iterations":1,"variables":[{"name":"value","type":"int"}],"body":{"nodes":[{"id":"noop","kind":"builtin.constant","config":{"value":0}}]}}}]});
    let error = compile_definition(&decode(&native), &registry)
        .unwrap_err()
        .to_string();
    assert!(error.contains("inline body and body parameter"), "{error}");
}

#[cfg(feature = "codegen")]
#[test]
fn public_codegen_preserves_single_callbacks_and_readonly_contracts() {
    let readonly = json!({"version":"2026-10-31","dependencies":{},"nodes":[
        {"id":"inspect","kind":"test.subgraph_inspect","subgraphs":{"child":{"source":{"node":{"id":"effect","kind":"test.subgraph_effect"}},"readonly":true}}}
    ],"outputs":[{"name":"value","node":"inspect","port":"value"}]});
    let registry = NodeRegistry::from_inventory().unwrap();
    EFFECTS.store(0, Ordering::SeqCst);
    for document in [
        node_parent(node_source()),
        flow_parent(flow_source()),
        readonly,
    ] {
        let plan = compile_definition(&decode(&document), &registry).unwrap();
        let expected = plan.manifest(&registry).unwrap();
        let artifacts = plan.generate_runner_execution_plans(&registry).unwrap();
        let actual = mf_runtime::WorkflowManifest::from_bytes(&artifacts.manifest_bytes).unwrap();
        assert_eq!(actual, expected);
        let frozen = mf_compiler::plan_definition(&actual.definition).unwrap();
        let source = frozen.generate_runner_artifacts().unwrap();
        assert_eq!(
            mf_compiler::CompiledWorkflow::from_json(&source.plan_json).unwrap(),
            frozen
        );
    }
    assert_eq!(EFFECTS.load(Ordering::SeqCst), 0);
}

#[test]
fn iteration_cannot_hide_loop_containers_in_named_callable_arguments() {
    let loop_source = json!({"source":{"node":{"id":"repeat","kind":"workflow.loop","loop":{"max_iterations":1,"variables":[{"name":"value","type":"int"}],"body":{"nodes":[{"id":"noop","kind":"builtin.constant","config":{"value":0}}]}}}},"inputs":{"x":"value"}});
    let registry = NodeRegistry::from_inventory().unwrap();
    for source in [
        loop_source.clone(),
        json!({"definition":node_parent(loop_source)}),
    ] {
        let document = json!({"version":"2026-10-31","dependencies":{},"nodes":[
            {"id":"items","kind":"builtin.constant","config":{"value":[]}},
            {"id":"each","kind":"builtin.iteration","config":{"input_mode":"arguments","body":{"nodes":[{"id":"invoke","kind":"builtin.dispatch","subgraphs":{"branches":{"collection":{"loop":source}}}}],"result":{"node":"invoke","port":"result"}}},"inputs":{"items":{"from":{"node":"items","port":"value"}}}}
        ]});
        let error = compile_definition(&decode(&document), &registry)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("cannot contain Loop or Iteration"),
            "{error}"
        );
    }
}
