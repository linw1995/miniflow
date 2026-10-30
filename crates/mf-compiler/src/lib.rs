mod calls;
mod flow_sources;
pub use flow_sources::{
    FlowResolutionError, ResolvedWorkflow, load_embedded_workflow, load_workflow, resolve_flows,
};
#[cfg(feature = "codegen")]
mod cache;
#[cfg(feature = "codegen")]
mod compatibility;
mod compiler;
mod construction;
#[cfg(feature = "codegen")]
mod dependency_project;
#[cfg(feature = "codegen")]
mod embedding;
#[cfg(feature = "codegen")]
mod inputs;
mod iteration;
pub use iteration::IterationValidationError;
mod loops;
#[cfg(feature = "codegen")]
mod pipeline;
mod plan;
#[cfg(feature = "codegen")]
mod state;
mod streaming;
#[cfg(feature = "codegen")]
mod typed_plan;

// Generated modules use the compiler's runtime so host dependency renames stay hygienic.
#[doc(hidden)]
pub use mf_runtime as __runtime;

/// Include a workflow emitted by the build-time `embed_workflows` helper.
///
/// The module exposes `plan`, `manifest`, and either `prepare_workflow` or `prepare_stream`.
/// The macro is available without the `codegen` feature; execution uses the host's runtime APIs.
#[macro_export]
macro_rules! embed_workflow {
    ($visibility:vis mod $name:ident) => {
        $visibility mod $name {
            use $crate as mf_compiler;
            use $crate::__runtime as mf_runtime;
            include!(concat!(env!("OUT_DIR"), "/mf-embedded/", stringify!($name), "/workflow.rs"));
        }
    };
}

#[cfg(feature = "codegen")]
pub use cache::{BuildDirectory, CacheError, default_build_directory};
#[cfg(feature = "codegen")]
pub use compatibility::{RuntimeCompatibilityError, validate_runtime_identity};
pub use compiler::{
    CyclePath, DescriptionError, TypeInferenceState, WorkflowCompileError, WorkflowExecutionError,
    compile_definition, compile_resolved_definition, describe_compiled, describe_interface,
    describe_workflow_inputs, execute_compiled, execute_compiled_with_inputs, instantiate_compiled,
    instantiate_compiled_with_observation, plan_definition, prepare_definition, resolve_execution,
    resolve_nodes, structural_order, topological_order, validate_definition,
};
#[cfg(feature = "codegen")]
pub use dependency_project::{
    DependencyProjectError, RunnerOptions, SupportPackages, write_dependency_project,
    write_dependency_project_with_options,
};
#[cfg(feature = "codegen")]
pub use embedding::{EmbedRequest, EmbeddingError, embed_workflows};
#[cfg(feature = "codegen")]
pub use inputs::{BuildInputs, InputError};
pub use mf_runtime::{
    CALL_KIND, CallConfig, ContextReference, ContextValue, ControlEdgeDefinition, DefinitionId,
    DefinitionParseError, EXIT_LOOP_KIND, EdgeDefinition, ExclusiveInputError, ExecutionContext,
    Flow, FlowConnection, FlowNode, FlowOutput, FlowOutputs, FlowSource, InputBindingDefinition,
    Inputs, LOOP_ASSIGN_KIND, LOOP_KIND, LOOP_SOURCE_ID, LoopBodyDefinition,
    LoopComparisonOperator, LoopConditionDefinition, LoopDefinition, LoopVariableDefinition,
    MAX_CALL_DEPTH, MAX_LOOP_DEPTH, MAX_LOOP_ITERATIONS, MAX_SCHEDULED_STEPS, NodeBuildError,
    NodeDefinition, NodeDependency, NodeExecutionError, NodeFactory, NodeId, NodeMetadata,
    NodePorts, NodeRegistration, NodeRegistry, NodeRegistryError, NodeResult, OrderEdgeDefinition,
    OutputDerivation, OutputDerivationError, OutputReferenceDefinition, Outputs, PortSpec,
    PreparedNode, RunObservation, TaskNode, TypeCompatibility, TypeDepthError, TypeMismatch,
    ValueKind, ValueRef, ValueType, WorkflowDefinition, WorkflowDefinitionVersion,
    WorkflowOutputDefinition, WorkflowRunError, deserialize_config, execute_node_in_context,
    output_id, select_context_output, validate_exclusive_inputs,
};
pub use mf_runtime::{IterationErrorPolicy, IterationMode};
pub use mf_runtime::{NodeExecution, PreparedStream, StreamExecution, StreamLimits};
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
#[cfg(feature = "codegen")]
pub use typed_plan::{
    TypedConnectionReport, TypedFallbackReason, TypedPlan, TypedSegment, plan_typed_segments,
};

pub use construction::{
    FlowBuildError, FlowBuilder, InvalidDefinitionSnafu as WorkflowInvalidDefinitionSnafu,
    InvalidEmbeddedConfigSnafu as WorkflowInvalidEmbeddedConfigSnafu,
    MetadataSnafu as WorkflowMetadataSnafu, NodeConstructionSnafu as WorkflowNodeConstructionSnafu,
    StreamBuildError, SubgraphSnafu as WorkflowSubgraphSnafu,
    UnknownKindSnafu as WorkflowUnknownKindSnafu, WorkflowBuildError, bind_workflow_inputs,
    build_flow, build_stream_program, instantiate_node_with_metadata,
    instantiate_subgraph_with_metadata, into_task, partition_execution_domains, prepare_stream,
    prepared_loop_assign_from_json, prepared_loop_source_from_json, stream_workflow_inputs,
    validate_execution, validate_stream_limits, validate_success_connection, workflow_input_schema,
};
pub use mf_runtime::{
    FlowRuntime, RuntimeOptions, TaskFlowNode, WorkflowInputError, WorkflowInputSchema,
};

mod dependencies;
pub use dependencies::{ExecutionDependency, FlowDependency};
