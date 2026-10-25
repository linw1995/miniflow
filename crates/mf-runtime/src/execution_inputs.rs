use crate::runner::{ContextSnafu, DependencySnafu, InputTypeSnafu};
use crate::workflow_inputs::{InvalidSnafu, TextValueSnafu, TypeMismatchSnafu, pointer};
use crate::{
    ContextValue, ExecutionDependency, FlowNode, Inputs, NodeExecutionError, WorkflowArguments,
    WorkflowInputError, WorkflowInputSchema, WorkflowRunError, output_id,
};
use mf_telemetry::event::{InputSelection, SkipCause};
use snafu::ResultExt;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    sync::Arc,
};

#[derive(Clone, Debug, Default)]
pub struct InputResolution {
    pub causes: BTreeSet<SkipCause>,
    pub selected: Vec<InputSelection>,
}

#[derive(Default)]
struct CandidateValues {
    produced: Option<(SkipCause, crate::ValueRef)>,
    skipped: Vec<SkipCause>,
}

/// Input state for one execution context, including its current startup binding.
#[derive(Clone, Debug, Default)]
pub struct ExecutionInputs {
    arguments: Arc<WorkflowArguments>,
    binding: Option<Arc<BoundInputs>>,
}

#[derive(Clone, Debug)]
struct BoundInputs {
    arguments: WorkflowArguments,
    nodes: BTreeSet<String>,
    stdin_owner: Option<String>,
}

impl ExecutionInputs {
    /// Replaces explicit arguments and invalidates every previously resolved source.
    pub fn set_arguments(&mut self, arguments: WorkflowArguments) {
        self.arguments = Arc::new(arguments);
        self.binding = None;
    }

    /// Rebinds from explicit arguments, so defaults and environment values cannot go stale.
    /// A failed binding leaves no startup inputs available to nodes.
    pub fn bind(
        &mut self,
        schema: &WorkflowInputSchema,
        environment: impl FnMut(&str) -> Option<OsString>,
        stdin_available: bool,
    ) -> Result<(), WorkflowInputError> {
        self.binding = None;
        let arguments = resolve_workflow_inputs(schema, &self.arguments, environment)?;
        schema.validate_stdin(&arguments, stdin_available)?;
        let stdin_owner = schema.stdin_owner(&arguments)?.map(str::to_owned);
        self.binding = Some(Arc::new(BoundInputs {
            arguments,
            nodes: schema.inputs.keys().cloned().collect(),
            stdin_owner,
        }));
        Ok(())
    }

    pub fn stdin_owner(&self) -> Option<&str> {
        self.binding.as_ref()?.stdin_owner.as_deref()
    }

    /// Startup arguments belong only to initial nodes in the top-level scope.
    pub fn initial<N>(&self, node: &FlowNode<N>, in_scope: bool) -> Inputs {
        let Some(binding) = &self.binding else {
            return Inputs::new();
        };
        let id = node.definition_id.as_str();
        if in_scope || !binding.nodes.contains(id) {
            return Inputs::new();
        }
        node.metadata
            .ports
            .inputs
            .iter()
            .filter_map(|port| {
                binding
                    .arguments
                    .0
                    .get(&output_id(id, &port.name))
                    .map(|value| (port.name.to_string(), value.clone()))
            })
            .collect()
    }

    /// Assembles and validates node inputs, retaining partial values for failure snapshots.
    ///
    /// Lookup returns `None` for an already transferred typed input.
    /// All dependencies are checked before honoring skips; skipped nodes bypass type validation.
    pub fn resolve_node<'a, 'value, N>(
        &self,
        node: &FlowNode<N>,
        in_scope: bool,
        dependencies: impl IntoIterator<Item = ExecutionDependency<'a>>,
        lookup: impl FnMut(
            ExecutionDependency<'a>,
        ) -> Result<Option<ContextValue<'value>>, NodeExecutionError>,
        inputs: &mut Inputs,
    ) -> Result<BTreeSet<SkipCause>, WorkflowRunError> {
        self.resolve_node_inputs(node, in_scope, dependencies, lookup, inputs)
            .map(|resolution| resolution.causes)
    }

    pub fn resolve_node_inputs<'a, 'value, N>(
        &self,
        node: &FlowNode<N>,
        in_scope: bool,
        dependencies: impl IntoIterator<Item = ExecutionDependency<'a>>,
        lookup: impl FnMut(
            ExecutionDependency<'a>,
        ) -> Result<Option<ContextValue<'value>>, NodeExecutionError>,
        inputs: &mut Inputs,
    ) -> Result<InputResolution, WorkflowRunError> {
        self.resolve_node_inputs_with_causes(
            node,
            in_scope,
            dependencies,
            lookup,
            inputs,
            BTreeSet::new(),
        )
    }

    pub fn resolve_node_inputs_with_causes<'a, 'value, N>(
        &self,
        node: &FlowNode<N>,
        in_scope: bool,
        dependencies: impl IntoIterator<Item = ExecutionDependency<'a>>,
        mut lookup: impl FnMut(
            ExecutionDependency<'a>,
        ) -> Result<Option<ContextValue<'value>>, NodeExecutionError>,
        inputs: &mut Inputs,
        mut causes: BTreeSet<SkipCause>,
    ) -> Result<InputResolution, WorkflowRunError> {
        *inputs = self.initial(node, in_scope);
        for (name, value) in inputs.iter_mut() {
            if node
                .metadata
                .input_bindings
                .get(name)
                .is_some_and(|binding| binding.secret)
            {
                *value = value.clone().into_secret();
            }
        }
        let id = node.definition_id.as_str();
        let mut selected = Vec::new();
        let mut groups: BTreeMap<&str, CandidateValues> = BTreeMap::new();
        for dependency in dependencies {
            let value = lookup(dependency.clone()).with_context(|_| DependencySnafu {
                definition_id: id,
                input: dependency.input.unwrap_or("<control>"),
            })?;
            if dependency.exclusive {
                let input = dependency.input.ok_or_else(|| {
                    ContextSnafu {
                        definition_id: id,
                        message: "exclusive dependency requires a target input",
                    }
                    .build()
                })?;
                let group = groups.entry(input).or_default();
                let source = SkipCause::Output {
                    source_node: dependency.source_node.into(),
                    source_output: dependency.source_output.into(),
                };
                match value {
                    Some(ContextValue::Value(value)) => {
                        group.produced = Some((source, value.clone()));
                    }
                    Some(ContextValue::Skipped) => {
                        group.skipped.push(source);
                    }
                    None => {
                        return ContextSnafu {
                            definition_id: id,
                            message: "exclusive candidate cannot use a transferred typed slot",
                        }
                        .fail();
                    }
                }
                continue;
            }
            match value {
                Some(ContextValue::Value(value)) => {
                    if let Some(input) = dependency.input {
                        let value = if node
                            .metadata
                            .input_bindings
                            .get(input)
                            .is_some_and(|binding| binding.secret)
                        {
                            value.clone().into_secret()
                        } else {
                            value.clone()
                        };
                        inputs.insert(input.to_owned(), value);
                    }
                }
                Some(ContextValue::Skipped) => {
                    causes.insert(SkipCause::Output {
                        source_node: dependency.source_node.into(),
                        source_output: dependency.source_output.into(),
                    });
                }
                None => {}
            }
        }
        for (input, group) in groups {
            if let Some((source, value)) = group.produced {
                selected.push(InputSelection {
                    input: input.to_owned(),
                    source_node: source.source_node().into(),
                    source_output: source
                        .source_output()
                        .expect("exclusive output source")
                        .into(),
                });
                let value = if node
                    .metadata
                    .input_bindings
                    .get(input)
                    .is_some_and(|binding| binding.secret)
                {
                    value.into_secret()
                } else {
                    value
                };
                inputs.insert(input.to_owned(), value);
            } else {
                causes.extend(group.skipped);
            }
        }
        if causes.is_empty() {
            for (name, value) in inputs {
                let port = node
                    .metadata
                    .ports
                    .inputs
                    .iter()
                    .find(|port| port.name == *name)
                    .ok_or_else(|| {
                        ContextSnafu {
                            definition_id: id,
                            message: format!("received undeclared input `{name}`"),
                        }
                        .build()
                    })?;
                port.value_type
                    .validate_shared(value)
                    .with_context(|_| InputTypeSnafu {
                        definition_id: id,
                        input: name,
                    })?;
            }
        }
        Ok(InputResolution { causes, selected })
    }
}

/// Resolves startup sources without reading process state directly.
///
/// Explicit arguments take precedence over environment values, then defaults.
/// Lookup is lazy: a present argument bypasses its environment source, and only
/// an absent environment value permits a default. Supplied values remain strict.
pub fn resolve_workflow_inputs(
    schema: &WorkflowInputSchema,
    arguments: &WorkflowArguments,
    mut environment: impl FnMut(&str) -> Option<OsString>,
) -> Result<WorkflowArguments, WorkflowInputError> {
    schema.validate(arguments)?;
    let mut resolved = arguments.clone();
    let mut environment_bytes = 0usize;
    for (node, ports) in &schema.inputs {
        for (port, input) in ports {
            let name = crate::output_id(node, port);
            if !resolved.0.contains_key(&name) {
                let environment = input.binding.env.as_deref().and_then(&mut environment);
                let value = if let Some(value) = environment {
                    // Avoid VarError::NotUnicode, which retains the raw environment value.
                    let text = match value.into_string() {
                        Ok(text) => text,
                        Err(_) => {
                            return InvalidSnafu {
                                path: pointer("", &name),
                                message: "environment value is not valid Unicode",
                            }
                            .fail();
                        }
                    };
                    environment_bytes = environment_bytes.saturating_add(text.len());
                    WorkflowArguments::check_size(environment_bytes)?;
                    Some(input.parse_text(&text).context(TextValueSnafu {
                        path: pointer("", &name),
                    })?)
                } else {
                    input.binding.default.clone().map(Into::into)
                };
                if let Some(value) = value {
                    input
                        .value_type
                        .validate_shared(&value)
                        .context(TypeMismatchSnafu {
                            path: pointer("", &name),
                        })?;
                    resolved.0.insert(name.clone(), value);
                }
            }
            if input.required && !resolved.0.contains_key(&name) {
                return InvalidSnafu {
                    path: pointer("", &name),
                    message: "required input was not provided by arguments, environment, or default",
                }.fail();
            }
        }
    }
    schema.mark_secrets(&mut resolved);
    Ok(resolved)
}
