use crate::call::{ArgumentTypeSnafu, MissingArgumentSnafu, UnknownArgumentSnafu};
use crate::{
    CallArguments, CallBindingId, CallContractError, CallInputSource, ResolvedCallBinding,
    ValueType,
};
use serde::Serialize;
use snafu::{OptionExt, ResultExt, ensure};
use std::collections::BTreeMap;

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
