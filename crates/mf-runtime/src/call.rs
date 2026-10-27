//! Contracts for directed calls, separate from ordinary output publication.
//!
//! These types do not schedule nodes. A dispatcher must consume resolved bindings, validate
//! composed inputs, allocate invocation identities, and execute through runtime boundaries.

use crate::{
    FlowDependency, Inputs, NodeId, NodeMetadata, NodeValues, Outputs, PortSpec, SkipCause,
    WorkflowRunError,
};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::collections::BTreeSet;

/// An index into one caller's prepared binding table in its owning scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CallBindingId(usize);

impl CallBindingId {
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0
    }
}

/// Correlates one request and response within a workflow execution.
///
/// The dispatcher must allocate a distinct invocation number for every call in that execution,
/// including nested calls and repeated calls from stream frames. Binding indexes alone are local
/// to a caller and cannot distinguish invocations. This identity is not an idempotency key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CallId {
    pub binding: CallBindingId,
    pub invocation: u64,
}

/// One caller output mapped to an exact target input name, without field-path interpretation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallPortMapping {
    pub output: String,
    pub input: String,
}

/// How a callable group handles target inputs not claimed by explicit port connections.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CallInputDefaults {
    /// Only explicitly mapped caller outputs supply arguments.
    #[default]
    ExplicitOnly,
    /// A node-level invocation connection assigns remaining inputs to the caller.
    RemainingInputs,
}

/// A declaration of one directed call and its atomic group of caller outputs.
///
/// Construction checks local names and duplicate target mappings only. Preparation must still
/// check port existence, types, external sources, alias uniqueness, and target eligibility.
/// Ordinary data/control edges do not become callable by constructing this declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallBinding {
    name: String,
    target: NodeId,
    ports: Vec<CallPortMapping>,
    defaults: CallInputDefaults,
}

impl CallBinding {
    pub fn new(
        name: impl Into<String>,
        target: NodeId,
        ports: Vec<CallPortMapping>,
        defaults: CallInputDefaults,
    ) -> Result<Self, CallContractError> {
        let name = name.into();
        ensure!(!name.trim().is_empty(), EmptyNameSnafu { field: "binding" });
        let mut inputs = BTreeSet::new();
        for port in &ports {
            ensure!(!port.output.is_empty(), EmptyNameSnafu { field: "output" });
            ensure!(!port.input.is_empty(), EmptyNameSnafu { field: "input" });
            ensure!(
                inputs.insert(&port.input),
                DuplicateInputSnafu { input: &port.input }
            );
        }
        Ok(Self {
            name,
            target,
            ports,
            defaults,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn target(&self) -> NodeId {
        self.target
    }

    pub fn ports(&self) -> &[CallPortMapping] {
        &self.ports
    }

    pub const fn defaults(&self) -> CallInputDefaults {
        self.defaults
    }
}

/// All caller-supplied values for one invocation, submitted together.
///
/// The namespaces remain separate: an explicit source output name may equal an unrelated
/// default target input name. External producer values are composed later by the runtime.
/// Values retain sharing and sensitivity; omission is not represented by JSON null.
#[derive(Clone, Debug, Default)]
pub struct CallArguments {
    /// Values keyed by caller output names from explicit port mappings.
    pub outputs: NodeValues,
    /// Values keyed by target input names assigned through a node-level default connection.
    pub defaults: NodeValues,
}

/// An explicit target selection with an atomic argument group, including an empty group.
///
/// Requests are intermediate calls, not final `NodeResult` outputs or stream emissions.
/// Publishing ordinary outputs must not implicitly dispatch a request, and submitting a request
/// must not broadcast its values through ordinary data edges. Unselected bindings remain callable.
#[derive(Debug)]
pub struct CallRequest {
    pub id: CallId,
    pub arguments: CallArguments,
}

impl CallRequest {
    /// Completes this envelope without losing its request identity.
    ///
    /// This only constructs a response; it neither executes a target nor records completion.
    pub fn respond(self, outcome: CallOutcome) -> CallResponse {
        CallResponse {
            id: self.id,
            outcome,
        }
    }
}

/// A terminal outcome of one call, not a completion of the target in its enclosing DAG.
#[derive(Debug)]
pub enum CallOutcome {
    /// Validated target outputs. Omitted optional outputs remain absent.
    Succeeded(Outputs),
    /// The selected call could not execute because its dependencies were skipped.
    Skipped { causes: Vec<SkipCause> },
    /// Preserve the runtime error and its typed source chain for the caller's policy.
    Failed { source: WorkflowRunError },
}

/// A response that may arrive independently of other calls from the same Parent.
#[derive(Debug)]
pub struct CallResponse {
    pub id: CallId,
    pub outcome: CallOutcome,
}

impl CallResponse {
    /// Rejects a response for another binding or invocation before exposing its outcome.
    pub fn into_outcome(self, expected: CallId) -> Result<CallOutcome, CallContractError> {
        ensure!(
            self.id == expected,
            ResponseMismatchSnafu {
                expected,
                actual: self.id,
            }
        );
        Ok(self.outcome)
    }
}

#[derive(Debug, Snafu)]
pub enum CallContractError {
    #[snafu(display("invalid call output `{port}`: {reason}"))]
    InvalidOutput { port: String, reason: &'static str },
    #[snafu(display("call output `{port}`: {source}"))]
    OutputDepth {
        port: String,
        source: crate::TypeDepthError,
    },
    #[snafu(display("unknown call {namespace} argument `{name}`"))]
    UnknownArgument {
        namespace: &'static str,
        name: String,
    },
    #[snafu(display("missing required call {namespace} argument `{name}`"))]
    MissingArgument {
        namespace: &'static str,
        name: String,
    },
    #[snafu(display("call {namespace} argument `{name}`: {source}"))]
    ArgumentType {
        namespace: &'static str,
        name: String,
        #[snafu(source(from(crate::TypeMismatch, Box::new)))]
        source: Box<crate::TypeMismatch>,
    },
    #[snafu(display("call {field} name must not be empty"))]
    EmptyName { field: &'static str },
    #[snafu(display("call input `{input}` has multiple explicit caller mappings"))]
    DuplicateInput { input: String },
    #[snafu(display("call response identity {actual:?} does not match request {expected:?}"))]
    ResponseMismatch { expected: CallId, actual: CallId },
}

impl NodeMetadata {
    /// Validates the distinct call-output namespace without publishing it as ordinary data.
    pub fn validate_call_outputs(&self) -> Result<(), CallContractError> {
        let mut names = BTreeSet::new();
        for port in &self.call_outputs {
            ensure!(
                !port.name.is_empty() && names.insert(&port.name),
                InvalidOutputSnafu {
                    port: port.name.as_ref(),
                    reason: "names must be nonempty and unique",
                }
            );
            ensure!(
                !self
                    .output_ports()
                    .any(|ordinary| ordinary.name == port.name),
                InvalidOutputSnafu {
                    port: port.name.as_ref(),
                    reason: "ordinary and call outputs must not share a name",
                }
            );
            port.value_type.check_depth().context(OutputDepthSnafu {
                port: port.name.as_ref(),
            })?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallInputSource {
    /// An explicit caller output, including its independent source contract.
    Output(PortSpec),
    /// A target-named argument supplied through the node-level default connection.
    Default,
    /// One or more ordinary producers recorded in the binding's dependencies.
    External,
    /// An optional input with no binding.
    Omitted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCallInput {
    pub port: PortSpec,
    pub source: CallInputSource,
    pub secret: bool,
}

/// A compiler-validated, scope-local contract. It does not activate or execute its target.
///
/// Output dependencies retain ordinary data and control selection semantics; success dependencies
/// remain separate. Prerequisites name the direct producers whose completion a dispatcher must
/// arrange before invoking the target. Construct this through compiler binding resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCallBinding {
    pub id: CallBindingId,
    pub name: String,
    pub caller: NodeId,
    pub target: NodeId,
    pub inputs: Vec<ResolvedCallInput>,
    pub dependencies: Vec<FlowDependency>,
    pub success_dependencies: Vec<NodeId>,
    pub prerequisites: Vec<NodeId>,
}

impl ResolvedCallBinding {
    /// Validates and maps only caller-owned values. External resolution and execution are separate.
    ///
    /// No partial map is returned on failure. Requiredness is checked within this selected binding,
    /// not across other tools sharing the caller's output declarations.
    pub fn bind_arguments(&self, arguments: &CallArguments) -> Result<Inputs, CallContractError> {
        let mut outputs = BTreeSet::new();
        let mut defaults = BTreeSet::new();
        for input in &self.inputs {
            match &input.source {
                CallInputSource::Output(port) => {
                    outputs.insert(port.name.as_ref());
                }
                CallInputSource::Default => {
                    defaults.insert(input.port.name.as_ref());
                }
                CallInputSource::External | CallInputSource::Omitted => {}
            }
        }
        for (namespace, values, allowed) in [
            ("output", &arguments.outputs, outputs),
            ("default", &arguments.defaults, defaults),
        ] {
            for name in values.keys() {
                ensure!(
                    allowed.contains(name.as_str()),
                    UnknownArgumentSnafu { namespace, name }
                );
            }
        }
        let mut bound = Inputs::new();
        for input in &self.inputs {
            let (namespace, values, source) = match &input.source {
                CallInputSource::Output(port) => ("output", &arguments.outputs, port),
                CallInputSource::Default => ("default", &arguments.defaults, &input.port),
                CallInputSource::External | CallInputSource::Omitted => continue,
            };
            let value = values.get(source.name.as_ref());
            if source.required || input.port.required {
                value.context(MissingArgumentSnafu {
                    namespace,
                    name: source.name.as_ref(),
                })?;
            }
            let Some(value) = value else { continue };
            source
                .value_type
                .validate_shared(value)
                .context(ArgumentTypeSnafu {
                    namespace,
                    name: source.name.as_ref(),
                })?;
            input
                .port
                .value_type
                .validate_shared(value)
                .context(ArgumentTypeSnafu {
                    namespace: "input",
                    name: input.port.name.as_ref(),
                })?;
            let value = if input.secret {
                value.clone().into_secret()
            } else {
                value.clone()
            };
            bound.insert(input.port.name.to_string(), value);
        }
        Ok(bound)
    }
}
