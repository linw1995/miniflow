mod call;
mod call_interface;
mod cancellation;
mod config_template;
mod context;
mod definition;
mod execution_domains;
mod execution_inputs;
mod execution_outputs;
mod flow;
#[cfg(feature = "async")]
mod future_execution;
mod iteration;
mod loop_node;
mod node;
mod node_target;
mod number;
mod registry;
mod runner;
mod runner_arguments;
#[cfg(unix)]
mod runner_signals;
mod snapshot;
mod stream;
mod stream_instance;
mod stream_io;
mod stream_plan;
mod subgraph;
mod task_stream;
mod typed_inputs;
mod typed_outputs;
mod value;
mod worker;
mod workflow_inputs;
mod workflow_manifest;

pub use call::{
    CallArguments, CallBinding, CallBindingId, CallContractError, CallDefinition, CallDefinitions,
    CallId, CallInputDefaults, CallInputSource, CallOutcome, CallPortMapping, CallRequest,
    CallResponse, ResolvedCallBinding, ResolvedCallInput,
};
pub use call_interface::{CallInterface, CallInterfaceError, CallInterfaces, CallParameter};
#[cfg(feature = "async")]
pub use cancellation::Cancelled;
pub use cancellation::StreamCancellation;
pub use config_template::{ConfigTemplate, ConfigTemplateError};
pub use context::{
    ExecutionContext, ExecutionDependency, ExecutionScope, GeneratedNodeResult, NodeResult,
    TypedNodeResult, execute_generated_node_in_context, execute_node_in_context,
    execute_resolved_node_in_context, select_context_output,
};
pub use definition::{
    CALL_KIND, CallConfig, ClusterDefinition, ControlEdgeDefinition, DefinitionId,
    DefinitionParseError, EXIT_LOOP_KIND, EdgeDefinition, ExclusiveInputError, FlowSource,
    InputBindingDefinition, LOOP_ASSIGN_KIND, LOOP_KIND, LOOP_SOURCE_ID, LoopBodyDefinition,
    LoopComparisonOperator, LoopConditionDefinition, LoopDefinition, LoopVariableDefinition,
    MAX_CALL_DEPTH, MAX_LOOP_DEPTH, MAX_LOOP_ITERATIONS, MAX_SCHEDULED_STEPS, NodeDefinition,
    NodeDependency, OrderEdgeDefinition, OutputReferenceDefinition, WorkflowDefinition,
    WorkflowDefinitionVersion, WorkflowOutputDefinition, validate_exclusive_inputs,
};
pub use execution_domains::{ExecutionDomain, ExecutionDomains};
pub use execution_inputs::{ExecutionInputs, InputResolution, resolve_workflow_inputs};
pub use execution_outputs::{ContextValue, ExecutionOutputs, OutputVisibility};
pub use flow::{
    Flow, FlowConnection, FlowNode, FlowOutput, FlowOutputs, FlowPlan, FlowRuntime,
    GeneratedDomainExecutor, NodeId, RuntimeOptions, TaskFlowNode,
};
#[cfg(feature = "async")]
pub use future_execution::FutureExecutionError;
#[doc(hidden)]
pub use inventory::submit as __submit_builtin_flow;
pub use iteration::{
    ITERATION_INPUT_ID, ITERATION_INPUT_KIND, ITERATION_KIND, IterationBodyDefinition,
    IterationConfig, IterationErrorPolicy, IterationMode, IterationResultDefinition,
    iteration_input_flow_node,
};
pub use loop_node::{
    loop_variable_types, prepared_loop_assign, prepared_loop_exit, prepared_loop_source_types,
    prepared_scope_source,
};
pub use mf_runtime_derive::{NodeEnum, NodeInputs, NodeOutputs, NodeValue};
pub use mf_telemetry::event::{NodeIdentity, ScopePathEntry, SkipCause};
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
pub use registry::{
    BuiltinFlowRegistration, BuiltinFlowRegistry, BuiltinFlowRegistryError, NodeRegistry,
    NodeRegistryError,
};
pub use runner::{SuccessDependencySnafu, WorkflowRunError};
pub use stream::{
    BatchInfo, EventContext, EventEffects, EventEmission, EventNode, FlushReason, NodeEvent,
    StreamExecution, StreamLimits, StreamNode, TimerUpdate, TypedEventNode, TypedStreamNode,
    execute_typed_event,
};
pub use stream_instance::{
    MessageId, MonotonicClock, StreamClock, StreamDelivery, StreamError, StreamInstance,
    StreamMetrics, StreamOptions, StreamOutput, StreamSummary,
};
pub use stream_io::{StreamStdio, TextInput};
pub use stream_plan::{
    FlowDependency, GeneratedStreamDomainExecutor, MessageSchedule, PreparedStream,
    StreamEmissionBinding, StreamInputResolver, StreamMessageInitializer, StreamMessageProgram,
    StreamOutputSelector, StreamPlan, StreamProgram,
};
pub use subgraph::PreparedSubgraph;
pub use task_stream::{
    DiscardSink, EmissionSink, Emitter, TaskStreamExecutor, TaskStreamNode, TypedEmitter,
    TypedTaskStreamNode,
};
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
pub use worker::{
    ExecutionWorkers, RuntimeWorkerHandle, WorkerHandle, WorkerJob, WorkerPool, WorkerPoolError,
};

pub use snapshot::{NodeSnapshot, SnapshotEvent, SnapshotOutcome, SnapshotRecorder};

pub use workflow_inputs::{
    InputBinding, MAX_WORKFLOW_INPUT_BYTES, StdinRequirement, WorkflowArguments, WorkflowInput,
    WorkflowInputError, WorkflowInputSchema,
};

pub use runner_arguments::{
    RunnerArgumentError, RunnerArguments, RunnerInvocation, runner_help_command,
};
#[cfg(unix)]
pub use runner_signals::{RunnerSignalError, RunnerSignals};

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
