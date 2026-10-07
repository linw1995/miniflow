#[cfg(feature = "codegen")]
mod cache;
#[cfg(feature = "codegen")]
mod compatibility;
mod compiler;
mod construction;
#[cfg(feature = "codegen")]
mod dependency_project;
#[cfg(feature = "codegen")]
mod inputs;
mod iteration;
mod loops;
#[cfg(feature = "codegen")]
mod pipeline;
mod plan;
#[cfg(feature = "codegen")]
mod state;
mod streaming;

#[cfg(feature = "codegen")]
pub use cache::{BuildDirectory, CacheError, default_build_directory};
#[cfg(feature = "codegen")]
pub use compatibility::{RuntimeCompatibilityError, validate_runtime_identity};
pub use compiler::{
    CyclePath, DescriptionError, TypeInferenceState, WorkflowCompileError, WorkflowExecutionError,
    compile_definition, describe_compiled, describe_interface, describe_workflow_inputs,
    execute_compiled, execute_compiled_with_inputs, instantiate_compiled, plan_definition,
    resolve_nodes, structural_order, topological_order, validate_definition,
};
#[cfg(feature = "codegen")]
pub use dependency_project::{
    DependencyProjectError, RunnerOptions, SupportPackages, write_dependency_project,
    write_dependency_project_with_options,
};
#[cfg(feature = "codegen")]
pub use inputs::{BuildInputs, InputError};
pub use mf_runtime::{
    ContextReference, ContextValue, ControlEdgeDefinition, DefinitionId, DefinitionParseError,
    EXIT_LOOP_KIND, EdgeDefinition, ExecutionContext, ExecutionDependency, Flow, FlowConnection,
    FlowNode, FlowOutput, FlowOutputs, Inputs, LOOP_ASSIGN_KIND, LOOP_KIND, LOOP_SOURCE_ID,
    LoopBodyDefinition, LoopComparisonOperator, LoopConditionDefinition, LoopDefinition,
    LoopVariableDefinition, MAX_LOOP_DEPTH, MAX_LOOP_ITERATIONS, MAX_SCHEDULED_STEPS,
    NodeBuildError, NodeDefinition, NodeDependency, NodeExecutionError, NodeFactory, NodeId,
    NodeMetadata, NodePorts, NodeRegistration, NodeRegistry, NodeRegistryError, NodeResult,
    OutputDerivation, OutputDerivationError, Outputs, PortSpec, PreparedNode, RunObservation,
    TaskNode, TypeCompatibility, TypeDepthError, TypeMismatch, ValueKind, ValueRef, ValueType,
    WorkflowDefinition, WorkflowDefinitionVersion, WorkflowOutputDefinition, WorkflowRunError,
    deserialize_config, execute_node_in_context, output_id, select_context_output,
};
pub use mf_runtime::{
    FlowDependency, NodeExecution, PreparedStream, StreamExecution, StreamLimits, StreamMode,
};
pub use mf_runtime::{IterationErrorPolicy, IterationMode};
#[cfg(feature = "codegen")]
pub use pipeline::{
    CompileRequest, PipelineError, cargo_command, compile_project, compile_project_with_options,
    resolve_project,
};
pub use plan::{
    CompiledWorkflow, GeneratedExecutionArtifacts, GeneratedWorkflowArtifacts, PlanError,
};
#[cfg(feature = "codegen")]
pub use state::{BuildGuard, StateError, atomic_copy, atomic_write, write_if_changed};
pub use streaming::{instantiate_stream, start_stream};

pub use construction::{
    FlowBuildError, FlowBuilder, InvalidDefinitionSnafu as WorkflowInvalidDefinitionSnafu,
    MetadataSnafu as WorkflowMetadataSnafu, StreamBuildError,
    SubgraphSnafu as WorkflowSubgraphSnafu, WorkflowBuildError, bind_workflow_inputs, build_flow,
    build_message_domains, instantiate_node_with_metadata, instantiate_subgraph_with_metadata,
    into_task, partition_execution_domains, prepare_stream, prepared_loop_assign_from_json,
    prepared_loop_source_from_json, stream_workflow_inputs, validate_execution,
    validate_stream_limits, workflow_input_schema,
};
pub use mf_runtime::{
    FlowRuntime, RuntimeOptions, TaskFlowNode, WorkflowInputError, WorkflowInputSchema,
};
