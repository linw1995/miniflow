mod cancellation;
mod config_template;
mod context;
mod definition;
mod execution_domains;
mod execution_inputs;
mod flow;
#[cfg(feature = "async")]
mod future_execution;
mod iteration;
mod loop_node;
mod message_domain;
mod node;
mod node_target;
mod number;
mod registry;
mod runner;
mod runner_arguments;
mod snapshot;
mod stream;
mod stream_instance;
mod stream_io;
mod stream_plan;
mod subgraph;
mod typed_inputs;
mod typed_outputs;
mod value;
mod worker;
mod workflow_inputs;
mod workflow_manifest;

#[cfg(feature = "async")]
pub use cancellation::Cancelled;
pub use cancellation::StreamCancellation;
pub use config_template::{ConfigTemplate, ConfigTemplateError};
pub use context::{
    ContextValue, ExecutionContext, ExecutionDependency, ExecutionScope, GeneratedNodeResult,
    NodeResult, TypedNodeResult, execute_generated_node_in_context, execute_node_in_context,
    select_context_output,
};
pub use definition::{
    ControlEdgeDefinition, DefinitionId, DefinitionParseError, EXIT_LOOP_KIND, EdgeDefinition,
    ExclusiveInputError, InputBindingDefinition, LOOP_ASSIGN_KIND, LOOP_KIND, LOOP_SOURCE_ID,
    LoopBodyDefinition, LoopComparisonOperator, LoopConditionDefinition, LoopDefinition,
    LoopVariableDefinition, MAX_LOOP_DEPTH, MAX_LOOP_ITERATIONS, MAX_SCHEDULED_STEPS,
    NodeDefinition, NodeDependency, OrderEdgeDefinition, OutputReferenceDefinition,
    WorkflowDefinition, WorkflowDefinitionVersion, WorkflowOutputDefinition,
    validate_exclusive_inputs,
};
pub use execution_domains::{ExecutionDomain, ExecutionDomains};
pub use execution_inputs::{ExecutionInputs, InputResolution, resolve_workflow_inputs};
pub use flow::{
    Flow, FlowConnection, FlowNode, FlowOutput, FlowOutputs, FlowPlan, FlowRuntime,
    GeneratedDomainExecutor, NodeId, RuntimeOptions, TaskFlowNode,
};
#[cfg(feature = "async")]
pub use future_execution::FutureExecutionError;
pub use iteration::{
    ITERATION_INPUT_ID, ITERATION_INPUT_KIND, ITERATION_KIND, IterationBodyDefinition,
    IterationConfig, IterationErrorPolicy, IterationMode, IterationResultDefinition,
    iteration_input_flow_node,
};
pub use loop_node::{
    loop_variable_types, prepared_loop_assign, prepared_loop_exit, prepared_loop_source_types,
    prepared_scope_source,
};
pub use message_domain::MessageDomains;
pub use mf_runtime_derive::{NodeEnum, NodeInputs, NodeOutputs, NodeValue};
pub use mf_telemetry::event::NodeIdentity;
pub use mf_telemetry::observation::RunObservation;
pub use mf_telemetry::observation::StreamObservation;
pub use node::{
    ContextReference, ExecutionFailedSnafu as NodeExecutionFailedSnafu,
    FactoryFailedSnafu as NodeFactoryFailedSnafu, Inputs,
    InvalidSubgraphSnafu as NodeInvalidSubgraphSnafu, NodeBuildError, NodeExecution,
    NodeExecutionError, NodeFactory, NodeMetadata, NodePortContract, NodePorts, NodeRegistration,
    NodeValue, NodeValues, OutputDerivation, OutputDerivationError, Outputs,
    PluginFailedSnafu as NodePluginFailedSnafu, PortSpec, PreparedNode, TaskNode,
    TypeCompatibility, TypeDepthError, TypeMismatch, TypeMismatchDetails, TypedTaskNode, ValueType,
    deserialize_config, encode_typed_result, execute_typed_task, output_id, port_names_match,
    verify_generated_metadata,
};
pub use node::{
    TypedConstructor, TypedGeneration, TypedGenerationError, TypedNodeValue, TypedPort,
    TypedTaskHandle,
};
pub use node_target::{
    NodeTarget, NodeTargetError, NodeTargetKind, PreparedTaskTarget, target_ports,
};
pub use number::{compare_json_numbers, number_to_f64, number_to_i64, number_to_u64};
pub use registry::{NodeRegistry, NodeRegistryError};
pub use runner::WorkflowRunError;
pub use stream::{
    BatchInfo, EventContext, EventEffects, EventEmission, EventNode, FlushReason, NodeEvent,
    StreamExecution, StreamLimits, StreamMode, StreamNode, TimerUpdate, TypedEventNode,
    TypedStreamNode, execute_typed_event,
};
pub use stream_instance::{
    Emitter, MessageId, MonotonicClock, StreamClock, StreamDelivery, StreamError, StreamInstance,
    StreamMetrics, StreamOptions, StreamOutput, StreamSummary,
};
pub use stream_io::{StreamStdio, TextInput};
pub use stream_plan::{FlowDependency, PreparedStream, StreamPlan};
pub use subgraph::PreparedSubgraph;
pub use typed_inputs::{
    InputDecodeError, InputField, InputValue, NodeInputs, Nullable, Shared, decode_default_input,
    decode_input, decode_node_value, decode_object_value, decode_required_input,
    reject_unknown_inputs, unknown_enum_tag, unknown_enum_variant,
};

pub use typed_outputs::{
    NodeOutputs, OutputEncodeError, OutputField, OutputValue, RustValueType, TypedField,
    TypedValueCodec, encode_node_value, encode_object_value, encode_output, validate_typed_output,
};

pub use value::{ValueKind, ValueRef};
pub use worker::{RuntimeWorkerHandle, WorkerHandle, WorkerJob, WorkerPool, WorkerPoolError};

pub use snapshot::{
    NodeSnapshot, SNAPSHOT_VERSION, Snapshot, SnapshotEntry, SnapshotError, SnapshotEvent,
    SnapshotOutcome, SnapshotRecord, SnapshotRecorder, SnapshotRecording, SnapshotStore,
    ValueDefinition, ValueId,
};

pub use workflow_inputs::{
    InputBinding, MAX_WORKFLOW_INPUT_BYTES, StdinRequirement, WorkflowArguments, WorkflowInput,
    WorkflowInputError, WorkflowInputSchema,
};

pub use runner_arguments::{RunnerArgumentError, RunnerArguments, RunnerInvocation};

pub use workflow_inputs::{
    JsonSnafu as WorkflowInputJsonSnafu, TooLargeSnafu as WorkflowInputTooLargeSnafu,
    WorkflowInterface, WorkflowInterfaceVersion, from_unique_json as decode_unique_json,
};

pub use workflow_inputs::{
    InvalidSnafu as WorkflowInputInvalidSnafu, TypeDepthSnafu as WorkflowInputTypeDepthSnafu,
};

pub use workflow_manifest::{
    LoopExecutionOrder, MANIFEST_FRAMING_VERSION, MANIFEST_HEADER_BYTES, MANIFEST_MAGIC,
    MAX_MANIFEST_PADDING_BYTES, MAX_MANIFEST_PAYLOAD_BYTES, MAX_MANIFEST_SECTION_BYTES,
    WorkflowManifest, WorkflowManifestError, WorkflowManifestVersion, describe_definition,
};
