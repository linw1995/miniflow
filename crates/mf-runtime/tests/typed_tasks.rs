use mf_runtime::{
    ContextReference, ContextValue, ExecutionContext, ExecutionDependency, FlowNode,
    InputDecodeError, Inputs, NodeBuildError, NodeExecutionError, NodeInputs, NodeMetadata,
    NodeOutputs, NodePorts, NodeResult, OutputDerivation, Outputs, PortSpec, PreparedNode,
    StdinRequirement, TaskFlowNode, TaskNode, TypedNodeResult, TypedTaskNode, ValueRef, ValueType,
    WorkflowRunError, execute_node_in_context,
};
use serde_json::json;
use snafu::ResultExt;
use std::{error::Error, io};

#[derive(NodeInputs)]
struct EchoInputs {
    input: ValueRef,
    label: Option<String>,
}

#[derive(NodeOutputs)]
struct EchoOutputs {
    value: ValueRef,
}

#[derive(NodeOutputs)]
struct CountOutputs {
    value: i64,
}

struct Echo;
impl TypedTaskNode for Echo {
    type Input = EchoInputs;
    type Output = EchoOutputs;

    fn execute(
        &self,
        input: EchoInputs,
        ctx: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        let ContextValue::Value(source) = ctx.output("source.value")? else {
            panic!("source must be present");
        };
        assert!(input.input.ptr_eq(source));
        assert!(input.label.is_none());
        Ok(EchoOutputs { value: input.input }.into())
    }
}

struct Emit(NodeResult);
impl TaskNode for Emit {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        Ok(self.0.clone())
    }
}

fn source(result: NodeResult, ctx: &mut ExecutionContext) {
    let node = FlowNode::new(
        "source",
        PreparedNode::from_parts(
            mf_runtime::NodeExecution::Task(Box::new(Emit(result))),
            NodePorts {
                inputs: vec![],
                outputs: vec![PortSpec::new("value", ValueType::Any, false)],
            },
        ),
    )
    .into_task()
    .unwrap();
    execute_node_in_context(&node, &[], ctx).unwrap();
}

fn dependency() -> ExecutionDependency<'static> {
    ExecutionDependency {
        input: Some("input"),
        source_node: "source",
        source_output: "value",
    }
}

fn echo() -> TaskFlowNode {
    FlowNode::new(
        "echo",
        PreparedNode::typed_task(Echo, NodePorts::default()).unwrap(),
    )
    .into_task()
    .unwrap()
}

struct NeverDecode;
impl NodeInputs for NeverDecode {
    fn ports() -> Vec<PortSpec> {
        vec![PortSpec::new("input", ValueType::Int64, true)]
    }

    fn from_inputs(_: Inputs) -> Result<Self, InputDecodeError> {
        panic!("input conversion must not run");
    }
}

struct NeverExecute;
impl TypedTaskNode for NeverExecute {
    type Input = NeverDecode;
    type Output = CountOutputs;

    fn execute(
        &self,
        _: NeverDecode,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        panic!("business execution must not run");
    }
}

#[test]
fn preparation_derives_ports_and_never_decodes_or_executes() {
    let metadata = NodeMetadata {
        typed_generation: None,
        ports: NodePorts::default(),
        output_derivations: vec![OutputDerivation::forward_input("value", "input")],
        context_references: vec![ContextReference::new("source.value", "source")],
        stdin: Some(StdinRequirement::UnlessInput("input".into())),
    };
    let prepared = PreparedNode::typed_task(NeverExecute, metadata.clone()).unwrap();
    assert_eq!(prepared.metadata.ports.inputs, NeverDecode::ports());
    assert_eq!(prepared.metadata.ports.outputs, CountOutputs::ports());
    assert_eq!(
        prepared.metadata.output_derivations,
        metadata.output_derivations
    );
    assert_eq!(
        prepared.metadata.context_references,
        metadata.context_references
    );
    assert_eq!(prepared.metadata.stdin, metadata.stdin);
    assert!(prepared.execution.as_task_node().is_some());
    let conflict = NodePorts {
        inputs: NeverDecode::ports(),
        outputs: vec![],
    };
    assert!(matches!(
        PreparedNode::typed_task(NeverExecute, conflict),
        Err(NodeBuildError::ConflictingInputDeclarations)
    ));
    let conflict = NodePorts {
        inputs: vec![],
        outputs: CountOutputs::ports(),
    };
    assert!(matches!(
        PreparedNode::typed_task(NeverExecute, conflict),
        Err(NodeBuildError::ConflictingOutputDeclarations)
    ));
}

#[test]
fn typed_execution_keeps_context_and_shared_payloads() {
    let root = ValueRef::from(json!({"nested": [1, 2]}));
    let mut ctx = ExecutionContext::default();
    source(
        Outputs::from([("value".into(), root.clone())]).into(),
        &mut ctx,
    );
    execute_node_in_context(&echo(), &[dependency()], &mut ctx).unwrap();
    let ContextValue::Value(output) = ctx.output("echo.value").unwrap() else {
        panic!("output was skipped")
    };
    assert!(output.ptr_eq(&root));
}

#[test]
fn skip_missing_dependencies_and_input_checks_precede_typed_decoding() {
    let node = FlowNode::new(
        "never",
        PreparedNode::typed_task(NeverExecute, NodePorts::default()).unwrap(),
    )
    .into_task()
    .unwrap();
    let mut ctx = ExecutionContext::default();
    source(
        NodeResult {
            skipped: ["value".into()].into(),
            ..Default::default()
        },
        &mut ctx,
    );
    let missing = ExecutionDependency {
        input: None,
        source_node: "missing",
        source_output: "value",
    };
    let error = execute_node_in_context(&node, &[dependency(), missing], &mut ctx).unwrap_err();
    assert!(matches!(error, WorkflowRunError::Dependency { .. }));
    execute_node_in_context(&node, &[dependency()], &mut ctx).unwrap();
    assert_eq!(ctx.output("never.value").unwrap(), ContextValue::Skipped);

    let mut ctx = ExecutionContext::default();
    source(
        Outputs::from([("value".into(), "wrong".into())]).into(),
        &mut ctx,
    );
    let error = execute_node_in_context(&node, &[dependency()], &mut ctx).unwrap_err();
    assert!(matches!(error, WorkflowRunError::InputType { .. }));
}

#[test]
fn decode_failures_retain_node_attribution_and_typed_sources() {
    let mut ctx = ExecutionContext::default();
    let error = execute_node_in_context(&echo(), &[], &mut ctx).unwrap_err();
    assert!(
        matches!(&error, WorkflowRunError::NodeExecution { definition_id, .. } if definition_id.as_str() == "echo")
    );
    let execution = error.source().unwrap();
    let decode = execution
        .source()
        .unwrap()
        .downcast_ref::<Box<InputDecodeError>>()
        .unwrap();
    assert!(matches!(decode.as_ref(), InputDecodeError::MissingField { port } if port == "input"));
    assert!(error.to_string().contains("/input"));

    let task = echo();
    let error = task
        .node
        .execute(
            Inputs::from([
                ("input".into(), ValueRef::null()),
                ("label".into(), ValueRef::null()),
            ]),
            &mut ctx,
        )
        .unwrap_err();
    let decode = error
        .source()
        .unwrap()
        .downcast_ref::<Box<InputDecodeError>>()
        .unwrap();
    assert_eq!(decode.pointer(), "/label");
    assert_eq!(
        decode
            .source()
            .unwrap()
            .downcast_ref::<mf_runtime::TypeMismatch>()
            .unwrap()
            .expected,
        ValueType::String
    );
}

struct BusinessFailure;
impl TypedTaskNode for BusinessFailure {
    type Input = EchoInputs;
    type Output = EchoOutputs;

    fn execute(
        &self,
        _: EchoInputs,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        let result: Result<TypedNodeResult<Self::Output>, Box<dyn Error + Send + Sync>> =
            Err(Box::new(io::Error::other("business sentinel")));
        result.context(mf_runtime::NodePluginFailedSnafu)
    }
}

#[test]
fn business_failures_keep_the_plugin_source_chain() {
    let prepared = PreparedNode::typed_task(BusinessFailure, NodePorts::default()).unwrap();
    let error = prepared
        .execution
        .into_task_node()
        .unwrap()
        .execute(
            Inputs::from([("input".into(), ValueRef::null())]),
            &mut ExecutionContext::default(),
        )
        .unwrap_err();
    assert!(matches!(error, NodeExecutionError::PluginFailed { .. }));
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .to_string(),
        "business sentinel"
    );
}

#[derive(NodeInputs)]
struct NoInputs {}

#[derive(NodeOutputs)]
struct BranchOutputs {
    selected: Option<ValueRef>,
    other: Option<ValueRef>,
}

struct Branch;
impl TypedTaskNode for Branch {
    type Input = NoInputs;
    type Output = BranchOutputs;

    fn execute(
        &self,
        _: NoInputs,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        Ok(TypedNodeResult {
            outputs: BranchOutputs {
                selected: Some(ValueRef::null()),
                other: None,
            },
            skipped: ["other".into()].into(),
            loop_summary: Some(mf_telemetry::event::LoopSummary {
                pass_count: mf_telemetry::Count::try_from(3).unwrap(),
                reason: mf_telemetry::event::LoopStopReason::Maximum,
            }),
        })
    }
}

#[test]
fn output_encoding_preserves_explicit_skips_null_and_loop_summary() {
    let prepared = PreparedNode::typed_task(Branch, NodePorts::default()).unwrap();
    let result = prepared
        .execution
        .as_task_node()
        .unwrap()
        .execute(Inputs::new(), &mut ExecutionContext::default())
        .unwrap();
    assert!(result.outputs["selected"].is_null());
    assert!(!result.outputs.contains_key("other"));
    assert_eq!(result.skipped, ["other".into()].into());
    assert_eq!(result.loop_summary.unwrap().pass_count.get(), 3);
    let node = FlowNode::new("branch", prepared).into_task().unwrap();
    let mut ctx = ExecutionContext::default();
    execute_node_in_context(&node, &[], &mut ctx).unwrap();
    assert_eq!(ctx.output("branch.other").unwrap(), ContextValue::Skipped);
    assert!(
        matches!(ctx.output("branch.selected").unwrap(), ContextValue::Value(value) if value.is_null())
    );
}

#[derive(NodeOutputs)]
struct Ratios {
    valid: i64,
    #[output(rename = "ratios./~")]
    ratios: Vec<f64>,
}

struct NonFinite;
impl TypedTaskNode for NonFinite {
    type Input = NoInputs;
    type Output = Ratios;

    fn execute(
        &self,
        _: NoInputs,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        Ok(Ratios {
            valid: 1,
            ratios: vec![1.0, f64::NAN],
        }
        .into())
    }
}

#[test]
fn encoding_failure_keeps_node_attribution_and_publishes_no_partial_outputs() {
    let node = FlowNode::new(
        "nonfinite",
        PreparedNode::typed_task(NonFinite, NodePorts::default()).unwrap(),
    )
    .into_task()
    .unwrap();
    let mut ctx = ExecutionContext::default();
    let error = execute_node_in_context(&node, &[], &mut ctx).unwrap_err();
    assert!(
        matches!(&error, WorkflowRunError::NodeExecution { definition_id, .. } if definition_id.as_str() == "nonfinite")
    );
    let encode = error
        .source()
        .unwrap()
        .source()
        .unwrap()
        .downcast_ref::<Box<mf_runtime::OutputEncodeError>>()
        .unwrap();
    assert_eq!(encode.pointer(), "/ratios.~1~0/1");
    assert_eq!(
        encode
            .source()
            .unwrap()
            .downcast_ref::<mf_runtime::TypeMismatch>()
            .unwrap()
            .expected,
        ValueType::Float64
    );
    assert!(ctx.output("nonfinite.valid").is_err());
    assert!(ctx.output("nonfinite.ratios./~").is_err());
}

#[test]
fn type_evidence_does_not_override_reflected_ports() {
    let derivations = vec![OutputDerivation::known_type("value", ValueType::Int64)];
    let prepared = PreparedNode::typed_task(
        NeverExecute,
        NodeMetadata {
            output_derivations: derivations.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        prepared.metadata.ports,
        NodePorts::from_types::<NeverDecode, CountOutputs>()
    );
    assert_eq!(prepared.metadata.output_derivations, derivations);
    prepared
        .metadata
        .ports
        .validate_derivations("typed", &derivations)
        .unwrap();
    for derivation in [
        OutputDerivation::known_type("other", ValueType::Int64),
        OutputDerivation::known_type("value", ValueType::Any),
        OutputDerivation::known_type("value", ValueType::String),
    ] {
        assert!(
            prepared
                .metadata
                .ports
                .validate_derivations("typed", &[derivation])
                .is_err()
        );
    }
}
