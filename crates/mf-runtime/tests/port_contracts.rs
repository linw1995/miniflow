use mf_runtime::{
    Emitter, EventContext, EventEffects, EventNode, ExecutionContext, Inputs, NodeBuildError,
    NodeEvent, NodeExecutionError, NodeMetadata, NodePortContract, NodePorts, NodeResult,
    NodeValue, OutputDerivation, PreparedNode, StdinRequirement, StreamNode, TaskNode,
    TypedEventNode, TypedNodeResult, TypedStreamNode, ValueType,
};
use std::collections::BTreeMap;

#[derive(NodeValue)]
struct Request {
    #[value(rename = "path./~")]
    path: Option<String>,
    count: i64,
}

#[derive(NodeValue)]
struct Response {
    rows: Vec<BTreeMap<String, i64>>,
    label: Option<String>,
}

struct NeverRun;

impl TaskNode for NeverRun {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        panic!("reflection must not execute tasks")
    }
}

impl EventNode for NeverRun {
    fn on_event(
        &mut self,
        _: NodeEvent,
        _: &EventContext<'_>,
    ) -> Result<EventEffects, NodeExecutionError> {
        panic!("reflection must not dispatch events")
    }
}

impl StreamNode for NeverRun {
    fn execute(
        &mut self,
        _: Inputs,
        _: &mut ExecutionContext,
        _: &mut Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        panic!("reflection must not run producers")
    }
}

impl NodePortContract for NeverRun {
    fn ports(&self) -> NodePorts {
        NodePorts::from_types::<Request, Response>()
    }
}

impl TypedEventNode for NeverRun {
    type Input = Request;
    type Output = Response;

    fn on_event(
        &mut self,
        _: NodeEvent<Request>,
        _: &EventContext<'_>,
    ) -> Result<EventEffects<Response>, NodeExecutionError> {
        panic!("typed preparation must not dispatch events")
    }
}

impl TypedStreamNode for NeverRun {
    type Input = Request;
    type Output = Response;

    fn execute(
        &mut self,
        _: Request,
        _: &mut ExecutionContext,
        _: &mut dyn FnMut(TypedNodeResult<Response>) -> Result<(), NodeExecutionError>,
    ) -> Result<(), NodeExecutionError> {
        panic!("typed preparation must not run producers")
    }
}

#[test]
fn every_execution_kind_reflects_both_contracts_and_preserves_other_metadata() {
    let metadata = NodeMetadata {
        stdin: Some(StdinRequirement::UnlessInput("path./~".into())),
        output_derivations: vec![OutputDerivation::known_type(
            "rows",
            ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Int64)))),
        )],
        ..Default::default()
    };
    let expected = NodePorts {
        inputs: Request::ports(),
        outputs: Response::ports(),
    };
    for prepared in [
        PreparedNode::new(NeverRun, metadata.clone()),
        PreparedNode::event(NeverRun, metadata.clone()),
        PreparedNode::stream(NeverRun, metadata.clone()),
        PreparedNode::typed_event(NeverRun, metadata.clone()),
        PreparedNode::typed_stream(NeverRun, metadata.clone()),
    ] {
        let prepared = prepared.unwrap();
        assert_eq!(prepared.metadata.ports, expected);
        assert_eq!(prepared.metadata.stdin, metadata.stdin);
        assert_eq!(
            prepared.metadata.output_derivations,
            metadata.output_derivations
        );
    }
}

#[test]
fn reflection_rejects_even_identical_factory_port_declarations() {
    let constructors: [fn(NodeMetadata) -> Result<PreparedNode, NodeBuildError>; 5] = [
        |metadata| PreparedNode::new(NeverRun, metadata),
        |metadata| PreparedNode::event(NeverRun, metadata),
        |metadata| PreparedNode::stream(NeverRun, metadata),
        |metadata| PreparedNode::typed_event(NeverRun, metadata),
        |metadata| PreparedNode::typed_stream(NeverRun, metadata),
    ];
    let ports = NodePorts {
        inputs: Request::ports(),
        outputs: Response::ports(),
    };
    for constructor in constructors {
        assert!(matches!(
            constructor(NodeMetadata::new(NodePorts {
                inputs: ports.inputs.clone(),
                outputs: vec![],
            })),
            Err(NodeBuildError::ConflictingInputDeclarations)
        ));
        assert!(matches!(
            constructor(NodeMetadata::new(NodePorts {
                inputs: vec![],
                outputs: ports.outputs.clone(),
            })),
            Err(NodeBuildError::ConflictingOutputDeclarations)
        ));
    }
}
