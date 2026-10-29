use crate::{
    CallArguments, CallBindingId, CallId, CallResponse, DefinitionId, ExecutionContext,
    OutputVisibility, ResolvedCallBinding, SkipCause, WorkflowRunError,
};
use snafu::Snafu;
use std::{collections::BTreeSet, sync::Arc, time::Instant};

/// A compiler-bound success prerequisite, evaluated against the invocation's input snapshot.
pub type CallGuard =
    dyn Fn(&ExecutionContext) -> Result<BTreeSet<SkipCause>, WorkflowRunError> + Send + Sync;

/// One executable binding in a compiler-validated, scope-local call table.
///
/// The owning Flow must exclude the target from ordinary execution domains. Visibility includes
/// external inputs and all evidence needed by nested calls and success predicates.
pub struct PreparedCall {
    pub target: crate::NodeIdentity,
    pub binding: ResolvedCallBinding,
    pub visibility: OutputVisibility,
    pub guard: Arc<CallGuard>,
}

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum CallDispatchError {
    #[snafu(display("directed calls are unavailable outside an active executable caller"))]
    Unavailable,
    #[snafu(display("caller `{caller}` has no executable binding {binding:?}"))]
    UnknownBinding {
        caller: DefinitionId,
        binding: CallBindingId,
    },
    #[snafu(transparent)]
    Arguments { source: crate::CallContractError },
    #[snafu(display("invalid prepared call table: {message}"))]
    InvalidPlan { message: &'static str },
    #[snafu(transparent)]
    Interfaces { source: crate::CallInterfaceError },
    #[snafu(display("directed call nesting exceeds the execution scope limit"))]
    DepthLimit,
    #[snafu(display("directed call invocation identity is exhausted"))]
    IdentityLimit,
    #[snafu(display("directed call deadline exceeded"))]
    DeadlineExceeded,
    #[snafu(display("directed call cancelled: {source}"))]
    Cancelled { source: crate::StreamError },
    #[snafu(display("directed call execution stopped: {source}"))]
    Execution {
        #[snafu(source(from(WorkflowRunError, Box::new)))]
        source: Box<WorkflowRunError>,
    },
}

impl From<CallDispatchError> for crate::NodeExecutionError {
    fn from(source: CallDispatchError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

pub trait CallDispatcher: Send + Sync {
    fn dispatch(
        &self,
        caller: &DefinitionId,
        id: CallId,
        arguments: CallArguments,
        context: &mut ExecutionContext,
        deadline: Option<Instant>,
    ) -> Result<CallResponse, CallDispatchError>;
}
