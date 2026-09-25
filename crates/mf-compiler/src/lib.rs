pub mod build;
pub mod compiler;
pub mod plan;
pub mod project;

pub use build::{BinaryBuildError, build_executable};
pub use compiler::{
    CyclePath, WorkflowCompileError, compile_definition, instantiate_compiled, resolve_nodes,
    topological_order, validate_definition,
};
pub use mf_runtime::*;
pub use plan::{CompiledWorkflow, GeneratedWorkflowArtifacts, PlanError};
pub use project::{RunnerProjectError, write_runner_project};

pub fn plugin_registry() -> Result<NodeRegistry, NodeRegistryError> {
    mf_bundle::registry()
}
