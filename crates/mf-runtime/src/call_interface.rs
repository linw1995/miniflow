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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    schemas: Vec<crate::ValueSchema>,
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
    pub fn schema_constraints(&self) -> &[crate::ValueSchema] {
        &self.schemas
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
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
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
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Validates decoded refinements as well as port types. Opaque schemas require separate admission.
    pub fn validate_decoded_arguments(
        &self,
        arguments: &CallArguments,
    ) -> Result<(), CallContractError> {
        self.validate_arguments(arguments)?;
        for (namespace, values, parameters) in [
            ("output", &arguments.outputs, &self.outputs),
            ("default", &arguments.defaults, &self.defaults),
        ] {
            for parameter in parameters {
                if let Some(value) = values.get(&parameter.name) {
                    for schema in &parameter.schemas {
                        schema.validate_shared(value).context(ArgumentTypeSnafu {
                            namespace,
                            name: &parameter.name,
                        })?;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn outputs(&self) -> &[CallParameter] {
        &self.outputs
    }
    pub fn defaults(&self) -> &[CallParameter] {
        &self.defaults
    }

    /// Checks caller-owned port types without reading or mapping target inputs.
    /// Strict codec refinements are available through `validate_decoded_arguments`.
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
                    schemas: Vec::new(),
                    secret: false,
                });
            parameter.required |= source.required || input.port.required;
            parameter.secret |= input.secret;
            if let Some(schema) = &input.schema
                && !parameter.schemas.contains(schema)
            {
                parameter.schemas.push(schema.clone());
            }

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
                    parameter.schemas.sort_by_cached_key(|schema| {
                        serde_json::to_string(schema).expect("validated schema serializes")
                    });
                    parameter
                })
                .collect()
        };
        CallInterface {
            id: self.id,
            name: self.name.clone(),
            description: self.description.clone(),
            outputs: normalize(outputs),
            defaults: normalize(defaults),
        }
    }
}

#[derive(Debug, Snafu)]
pub enum CallInterfaceError {
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
    #[snafu(display("scope has no task caller `{caller}` for its call interfaces"))]
    UnknownCaller { caller: DefinitionId },
}

/// Immutable discovery data for one prepared scope. Empty catalogs do not allocate.
#[derive(Clone, Default)]
pub struct CallInterfaces {
    callers: Option<Arc<BTreeMap<DefinitionId, Vec<CallInterface>>>>,
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
        })
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

    pub fn validate_callers<'a>(
        &self,
        available: impl IntoIterator<Item = &'a DefinitionId>,
    ) -> Result<(), CallInterfaceError> {
        let available: BTreeSet<_> = available.into_iter().collect();
        for caller in self.callers() {
            ensure!(
                available.contains(caller),
                UnknownCallerSnafu {
                    caller: caller.clone()
                }
            );
        }
        Ok(())
    }
}
