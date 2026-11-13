use crate::{DefinitionId, RootScopeRelations, ValueType};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use snafu::{ResultExt, Snafu, ensure};
use std::collections::BTreeMap;

/// Engine-owned declaration of invocation-local, typed mutable values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowVariablesDefinition {
    pub variables: Vec<WorkflowVariableDefinition>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowVariableDefinition {
    pub name: String,
    #[serde(rename = "type")]
    pub value_type: Value,
    // Require the field while accepting an explicit JSON null.
    #[serde(deserialize_with = "Value::deserialize")]
    pub initial: Value,
}

#[derive(Debug, Snafu)]
pub enum WorkflowVariablesError {
    #[snafu(display("variables must not be empty"))]
    EmptyVariables,
    #[snafu(display("invalid variable name `{variable}`"))]
    InvalidVariableName { variable: String },
    #[snafu(display("duplicate variable `{variable}`"))]
    DuplicateVariable { variable: String },
    #[snafu(display("variable `{variable}`: {source}"))]
    VariableType {
        variable: String,
        source: crate::TypeDescriptorError,
    },
    #[snafu(display("variable `{variable}`: {source}"))]
    TypeDepth {
        variable: String,
        source: crate::TypeDepthError,
    },
    #[snafu(display("initial values must match declared variable names"))]
    InitialNames,
    #[snafu(display("initial value for variable `{variable}`: {source}"))]
    InitialValue {
        variable: String,
        source: crate::TypeMismatch,
    },
}

impl WorkflowVariablesDefinition {
    pub fn into_scope(
        self,
        input: DefinitionId,
    ) -> Result<RootScopeRelations, WorkflowVariablesError> {
        let mut values = BTreeMap::new();
        let mut initial = BTreeMap::new();
        for variable in self.variables {
            let value_type =
                ValueType::parse_descriptor(&variable.value_type).context(VariableTypeSnafu {
                    variable: &variable.name,
                })?;
            ensure!(
                values.insert(variable.name.clone(), value_type).is_none(),
                DuplicateVariableSnafu {
                    variable: &variable.name
                }
            );
            initial.insert(variable.name, variable.initial);
        }
        let scope = RootScopeRelations {
            input,
            values,
            initial,
            writes: BTreeMap::new(),
        };
        scope.validate_values()?;
        Ok(scope)
    }
}

impl RootScopeRelations {
    /// Validate the stored schema independently of authoring syntax or providers.
    pub fn validate_values(&self) -> Result<(), WorkflowVariablesError> {
        ensure!(!self.values.is_empty(), EmptyVariablesSnafu);
        ensure!(
            self.values.keys().eq(self.initial.keys()),
            InitialNamesSnafu
        );
        for (variable, value_type) in &self.values {
            ensure!(
                !variable.trim().is_empty(),
                InvalidVariableNameSnafu { variable }
            );
            value_type
                .check_depth()
                .context(TypeDepthSnafu { variable })?;
            value_type
                .validate_value(&self.initial[variable])
                .context(InitialValueSnafu { variable })?;
        }
        Ok(())
    }
}
