use crate::{
    CallArguments, CallBindingId, CallId, CallResponse, DefinitionId, ExecutionContext,
    OutputVisibility, ResolvedCallBinding, SkipCause, WorkflowRunError,
};
use snafu::Snafu;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Instant,
};

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

/// Shared finite and streaming bindings; executors remain owned by their enclosing plan.
pub struct CallTable {
    callers: BTreeMap<crate::DefinitionId, Vec<crate::PreparedCall>>,
    interfaces: crate::CallInterfaces,
}

impl CallTable {
    pub fn interfaces(&self) -> &crate::CallInterfaces {
        &self.interfaces
    }

    pub fn prepared_calls(&self) -> impl Iterator<Item = &crate::PreparedCall> {
        self.callers.values().flatten()
    }

    pub fn new<N>(
        nodes: &[crate::FlowNode<N>],
        domains: &crate::ExecutionDomains,
        calls: Vec<crate::PreparedCall>,
    ) -> Result<Option<Self>, crate::CallDispatchError> {
        use crate::invocation::InvalidPlanSnafu;
        use snafu::{OptionExt, ensure};
        if calls.is_empty() {
            return Ok(None);
        }
        let mut callers: BTreeMap<crate::DefinitionId, Vec<crate::PreparedCall>> = BTreeMap::new();
        let scheduled: BTreeSet<_> = domains
            .domains()
            .iter()
            .flat_map(|domain| domain.nodes.iter().copied())
            .collect();
        for call in calls {
            let caller = nodes
                .get(call.binding.caller.index())
                .context(InvalidPlanSnafu {
                    message: "unknown caller",
                })?;
            let target = nodes
                .get(call.binding.target.index())
                .context(InvalidPlanSnafu {
                    message: "unknown target",
                })?;
            ensure!(
                target.definition_id.as_str() == call.target.id,
                InvalidPlanSnafu {
                    message: "target identity mismatch"
                }
            );
            ensure!(
                !scheduled.contains(&call.binding.target),
                InvalidPlanSnafu {
                    message: "call target remains in ordinary execution domains"
                }
            );
            ensure!(
                !call.binding.inputs.iter().any(|input| {
                    matches!(
                        input.source,
                        crate::CallInputSource::Output(_) | crate::CallInputSource::Default
                    ) && call.binding.dependencies.iter().any(|dependency| {
                        dependency.input.as_deref() == Some(input.port.name.as_ref())
                    })
                }),
                InvalidPlanSnafu {
                    message: "caller and external inputs overlap"
                }
            );
            let bindings = callers.entry(caller.definition_id.clone()).or_default();
            ensure!(
                call.binding.id.index() == bindings.len(),
                InvalidPlanSnafu {
                    message: "binding indexes must be contiguous and ordered"
                }
            );
            bindings.push(call);
        }
        let interfaces = crate::CallInterfaces::new(callers.iter().map(|(caller, calls)| {
            (
                caller.clone(),
                calls.iter().map(|call| call.binding.interface()).collect(),
            )
        }))?;
        Ok((!callers.is_empty()).then_some(Self {
            callers,
            interfaces,
        }))
    }

    pub fn dispatch<N>(
        &self,
        caller: &crate::DefinitionId,
        request: crate::CallRequest,
        context: &mut crate::ExecutionContext,
        deadline: Option<std::time::Instant>,
        nodes: &[crate::FlowNode<N>],
        invoke: impl FnOnce(
            &crate::FlowNode<N>,
            crate::Inputs,
            &mut crate::ExecutionContext,
        ) -> Result<crate::NodeResult, crate::NodeExecutionError>,
    ) -> Result<crate::CallResponse, crate::CallDispatchError> {
        use crate::invocation::{ExecutionSnafu, UnknownBindingSnafu};
        use snafu::{OptionExt, ResultExt};
        let calls = self;
        let crate::CallRequest { id, arguments } = request;
        let call = calls
            .callers
            .get(caller)
            .and_then(|bindings| bindings.get(id.binding.index()))
            .context(UnknownBindingSnafu {
                caller: caller.clone(),
                binding: id.binding,
            })?;
        let initial = call.binding.bind_arguments(&arguments)?;
        let node = &nodes[call.binding.target.index()];
        context.run_directed_call(
            id,
            &call.target,
            call.visibility.clone(),
            deadline,
            |child| {
                let mut skipped = Vec::new();
                let result = crate::execute_resolved_node_in_context(
                    node,
                    child,
                    |ctx, inputs| {
                        *inputs = initial;
                        let resolution = crate::ExecutionInputs::resolve_composed_inputs(
                            node,
                            call.binding
                                .dependencies
                                .iter()
                                .map(crate::FlowDependency::borrowed),
                            |dependency| ctx.output(&dependency.source_key).map(Some),
                            inputs,
                            (call.guard)(ctx)?,
                        )?;
                        skipped = resolution.causes.iter().cloned().collect();
                        Ok(resolution)
                    },
                    |inputs, ctx| {
                        invoke(node, inputs, ctx).map(crate::GeneratedNodeResult::Dynamic)
                    },
                );
                child.check_call_limits(deadline)?;
                if let Err(source) = result {
                    if source.is_terminal_control() {
                        return Err(source).context(ExecutionSnafu);
                    }
                    return Ok(crate::CallOutcome::Failed { source });
                }
                if !skipped.is_empty() {
                    return Ok(crate::CallOutcome::Skipped { causes: skipped });
                }
                let mut outputs = crate::Outputs::new();
                for (port, key) in node.metadata.ports.outputs.iter().zip(node.output_keys()) {
                    if let Ok(crate::ContextValue::Value(value)) = child.output(key) {
                        outputs.insert(port.name.to_string(), value.clone());
                    }
                }
                Ok(crate::CallOutcome::Succeeded(outputs))
            },
        )
    }
}
