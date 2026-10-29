use crate::call::{ArgumentTypeSnafu, MissingArgumentSnafu, UnknownArgumentSnafu};
use crate::{
    CallArguments, CallBindingId, CallContractError, CallInputSource, DefinitionId,
    ResolvedCallBinding, ValueType,
};
use serde::Serialize;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

/// One caller-owned parameter. Every listed constraint must hold; they are not alternatives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CallParameter {
    name: String,
    required: bool,
    constraints: Vec<ValueType>,
    secret: bool,
}

impl CallParameter {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn required(&self) -> bool {
        self.required
    }
    pub fn constraints(&self) -> &[ValueType] {
        &self.constraints
    }
    pub const fn is_secret(&self) -> bool {
        self.secret
    }
}

/// A read-only, provider-neutral interface, without target indexes, external inputs, or values.
///
/// Output and default namespaces remain separate, including when they contain the same name.
/// This is a runtime type contract, not a provider-specific JSON Schema or dispatch capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CallInterface {
    id: CallBindingId,
    name: String,
    outputs: Vec<CallParameter>,
    defaults: Vec<CallParameter>,
}

impl CallInterface {
    pub const fn id(&self) -> CallBindingId {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn outputs(&self) -> &[CallParameter] {
        &self.outputs
    }
    pub fn defaults(&self) -> &[CallParameter] {
        &self.defaults
    }

    /// Checks exactly the caller-visible constraints, without reading or mapping target inputs.
    pub fn validate_arguments(&self, arguments: &CallArguments) -> Result<(), CallContractError> {
        for (namespace, values, parameters) in [
            ("output", &arguments.outputs, &self.outputs),
            ("default", &arguments.defaults, &self.defaults),
        ] {
            for name in values.keys() {
                ensure!(
                    parameters
                        .binary_search_by(|p| p.name.as_str().cmp(name))
                        .is_ok(),
                    UnknownArgumentSnafu { namespace, name }
                );
            }
            for parameter in parameters {
                let value = values.get(&parameter.name);
                if parameter.required {
                    value.context(MissingArgumentSnafu {
                        namespace,
                        name: &parameter.name,
                    })?;
                }
                if let Some(value) = value {
                    for constraint in &parameter.constraints {
                        constraint
                            .validate_shared(value)
                            .context(ArgumentTypeSnafu {
                                namespace,
                                name: &parameter.name,
                            })?;
                    }
                }
            }
        }
        Ok(())
    }
}

impl ResolvedCallBinding {
    /// Projects an already resolved binding without retaining its private planning records.
    pub fn interface(&self) -> CallInterface {
        let mut outputs = BTreeMap::<String, CallParameter>::new();
        let mut defaults = BTreeMap::<String, CallParameter>::new();
        for input in &self.inputs {
            let (parameters, source) = match &input.source {
                CallInputSource::Output(source) => (&mut outputs, source),
                CallInputSource::Default => (&mut defaults, &input.port),
                CallInputSource::External | CallInputSource::Omitted => continue,
            };
            let parameter = parameters
                .entry(source.name.to_string())
                .or_insert_with(|| CallParameter {
                    name: source.name.to_string(),
                    required: false,
                    constraints: Vec::new(),
                    secret: false,
                });
            parameter.required |= source.required || input.port.required;
            parameter.secret |= input.secret;
            for constraint in [&source.value_type, &input.port.value_type] {
                if !parameter.constraints.contains(constraint) {
                    parameter.constraints.push(constraint.clone());
                }
            }
        }
        let normalize = |parameters: BTreeMap<String, CallParameter>| {
            parameters
                .into_values()
                .map(|mut parameter| {
                    parameter
                        .constraints
                        .sort_by_cached_key(ToString::to_string);
                    parameter
                })
                .collect()
        };
        CallInterface {
            id: self.id,
            name: self.name.clone(),
            outputs: normalize(outputs),
            defaults: normalize(defaults),
        }
    }
}

#[derive(Debug, Snafu)]
pub enum CallInterfaceError {
    #[snafu(display("node `{viewer}` references no executable call catalog `{owner}`"))]
    MissingSource {
        viewer: DefinitionId,
        owner: DefinitionId,
    },
    #[snafu(display("node `{viewer}` cannot both own calls and project another call catalog"))]
    MixedOwnership { viewer: DefinitionId },
    #[snafu(display("node `{viewer}` declares more than one call catalog source"))]
    DuplicateSource { viewer: DefinitionId },
    #[snafu(display("call interface caller name must not be blank"))]
    InvalidCaller,
    #[snafu(display("call interface caller `{caller}` is declared more than once"))]
    DuplicateCaller { caller: DefinitionId },
    #[snafu(display("caller `{caller}` has a blank call alias"))]
    InvalidAlias { caller: DefinitionId },
    #[snafu(display("caller `{caller}` has duplicate call binding ID {id}"))]
    DuplicateId { caller: DefinitionId, id: usize },
    #[snafu(display("caller `{caller}` has duplicate call alias `{name}`"))]
    DuplicateAlias { caller: DefinitionId, name: String },
}

/// Immutable discovery data for one prepared scope. Empty catalogs do not allocate.
#[derive(Clone, Default)]
pub struct CallInterfaces {
    callers: Option<Arc<BTreeMap<DefinitionId, Vec<CallInterface>>>>,
    views: Option<Arc<BTreeMap<DefinitionId, DefinitionId>>>,
}

impl fmt::Debug for CallInterfaces {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A context diagnostic must not dump interfaces belonging to other callers.
        f.debug_struct("CallInterfaces")
            .field("callers", &self.callers().count())
            .finish_non_exhaustive()
    }
}

impl CallInterfaces {
    pub fn new(
        entries: impl IntoIterator<Item = (DefinitionId, Vec<CallInterface>)>,
    ) -> Result<Self, CallInterfaceError> {
        let mut seen = BTreeSet::new();
        let mut callers = BTreeMap::new();
        for (caller, mut interfaces) in entries {
            ensure!(!caller.as_str().trim().is_empty(), InvalidCallerSnafu);
            ensure!(
                seen.insert(caller.clone()),
                DuplicateCallerSnafu {
                    caller: caller.clone()
                }
            );
            let mut ids = BTreeSet::new();
            let mut names = BTreeSet::new();
            for interface in &interfaces {
                ensure!(
                    !interface.name.trim().is_empty(),
                    InvalidAliasSnafu {
                        caller: caller.clone()
                    }
                );
                ensure!(
                    ids.insert(interface.id),
                    DuplicateIdSnafu {
                        caller: caller.clone(),
                        id: interface.id.index()
                    }
                );
                ensure!(
                    names.insert(&interface.name),
                    DuplicateAliasSnafu {
                        caller: caller.clone(),
                        name: &interface.name
                    }
                );
            }
            interfaces.sort_by_key(CallInterface::id);
            if !interfaces.is_empty() {
                callers.insert(caller, interfaces);
            }
        }
        Ok(Self {
            callers: (!callers.is_empty()).then(|| Arc::new(callers)),
            views: None,
        })
    }

    /// Resolves declared read-only projections against actual owners, never against other views.
    pub fn with_views(
        mut self,
        entries: impl IntoIterator<Item = (DefinitionId, DefinitionId)>,
    ) -> Result<Self, CallInterfaceError> {
        let mut views = BTreeMap::new();
        for (viewer, owner) in entries {
            ensure!(
                !self.for_caller(&owner).is_empty(),
                MissingSourceSnafu {
                    viewer: viewer.clone(),
                    owner,
                }
            );
            ensure!(
                self.for_caller(&viewer).is_empty(),
                MixedOwnershipSnafu {
                    viewer: viewer.clone(),
                }
            );
            ensure!(
                views.insert(viewer.clone(), owner).is_none(),
                DuplicateSourceSnafu { viewer }
            );
        }
        self.views = (!views.is_empty()).then(|| Arc::new(views));
        Ok(self)
    }

    pub fn for_node(&self, node: &DefinitionId) -> &[CallInterface] {
        let owner = self
            .views
            .as_ref()
            .and_then(|views| views.get(node))
            .unwrap_or(node);
        self.for_caller(owner)
    }

    pub fn is_empty(&self) -> bool {
        self.callers.is_none()
    }

    pub fn callers(&self) -> impl Iterator<Item = &DefinitionId> {
        self.callers.iter().flat_map(|callers| callers.keys())
    }

    pub fn for_caller(&self, caller: &DefinitionId) -> &[CallInterface] {
        self.callers
            .as_ref()
            .and_then(|callers| callers.get(caller))
            .map_or(&[], Vec::as_slice)
    }
}
