use crate::iteration::{body_definition, parse_config};
use crate::{DefinitionId, LoopDefinition, WorkflowDefinition};
use mf_runtime::{ITERATION_INPUT_KIND, ITERATION_KIND, IterationErrorPolicy, IterationMode};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::collections::{BTreeMap, BTreeSet};
use syn::LitStr;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledWorkflow {
    pub definition: WorkflowDefinition,
    pub execution_order: Vec<DefinitionId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedWorkflowArtifacts {
    pub rust_source: String,
    pub plan_json: String,
}

#[derive(Debug, Snafu)]
pub enum PlanError {
    #[snafu(display("could not serialize compiled workflow: {source}"))]
    Serialize { source: serde_json::Error },
    #[snafu(display("could not parse compiled workflow: {source}"))]
    Parse { source: serde_json::Error },
    #[snafu(display("compiled plan references unknown node `{definition_id}`"))]
    UnknownNode { definition_id: DefinitionId },
    #[snafu(display("compiled plan has an incomplete or duplicate execution order"))]
    InvalidExecutionOrder,
    #[snafu(display("compiled plan connects node `{from_node}` after node `{to_node}`"))]
    InvalidEdgeOrder {
        from_node: DefinitionId,
        to_node: DefinitionId,
    },
    #[snafu(display("compiled plan selects output name `{name}` more than once"))]
    DuplicateOutputName { name: String },
    #[snafu(display("generated Rust code is invalid: {source}"))]
    GeneratedSyntax { source: syn::Error },
    #[snafu(display("invalid embedded Loop node `{definition_id}`: {message}"))]
    InvalidLoopConfig {
        definition_id: DefinitionId,
        message: String,
    },
    #[snafu(display("could not generate iteration node `{definition_id}`: {message}"))]
    Iteration {
        definition_id: DefinitionId,
        message: String,
    },
}

impl CompiledWorkflow {
    pub fn start_observation(
        &self,
        observer: &mf_telemetry::observation::Observer,
        run_id: mf_telemetry::identity::RunId,
    ) -> Result<mf_runtime::RunObservation, mf_telemetry::ContractError> {
        if self.definition.version == mf_runtime::WorkflowDefinitionVersion::V2026_09_29 {
            let description = crate::describe_compiled(self).map_err(|error| {
                mf_telemetry::ContractError::Invalid {
                    message: error.to_string(),
                }
            })?;
            return observer.start_with_description(description, run_id);
        }
        if self.execution_order.len() != self.definition.nodes.len() {
            return Err(mf_telemetry::ContractError::Invalid {
                message: "observation order is incomplete".into(),
            });
        }
        let nodes_by_id: BTreeMap<_, _> = self
            .definition
            .nodes
            .iter()
            .map(|node| (&node.id, node))
            .collect();
        if nodes_by_id.len() != self.definition.nodes.len() {
            return Err(mf_telemetry::ContractError::Invalid {
                message: "duplicate observation node ID".into(),
            });
        }
        let order: Vec<String> = self
            .execution_order
            .iter()
            .map(ToString::to_string)
            .collect();
        let id = mf_telemetry::identity::WorkflowId::from_definition(&self.definition, &order)?;
        let nodes = self
            .execution_order
            .iter()
            .map(|id| {
                nodes_by_id
                    .get(id)
                    .map(|node| mf_telemetry::event::NodeIdentity {
                        id: id.to_string(),
                        kind: node.kind.clone(),
                        path: Vec::new(),
                    })
                    .ok_or_else(|| mf_telemetry::ContractError::Invalid {
                        message: format!("unknown observation node {id}"),
                    })
            })
            .collect::<Result<_, _>>()?;
        observer.start(id, run_id, nodes)
    }

    pub fn to_json(&self) -> Result<String, PlanError> {
        serde_json::to_string(self).context(SerializeSnafu)
    }

    pub fn from_json(input: &str) -> Result<Self, PlanError> {
        serde_json::from_str(input).context(ParseSnafu)
    }

    pub fn generate_artifacts(&self) -> Result<GeneratedWorkflowArtifacts, PlanError> {
        if self.execution_order.len() != self.definition.nodes.len() {
            return InvalidExecutionOrderSnafu.fail();
        }
        let nodes_by_id: BTreeMap<DefinitionId, _> = self
            .definition
            .nodes
            .iter()
            .map(|node| (node.id.clone(), node))
            .collect();
        let mut indices = BTreeMap::new();
        for (index, definition_id) in self.execution_order.iter().enumerate() {
            if !nodes_by_id.contains_key(definition_id)
                || indices.insert(definition_id.clone(), index).is_some()
            {
                return InvalidExecutionOrderSnafu.fail();
            }
        }

        for (from_node, to_node) in self
            .definition
            .edges
            .iter()
            .map(|edge| (&edge.from_node, &edge.to_node))
            .chain(
                self.definition
                    .control_edges
                    .iter()
                    .map(|edge| (&edge.from_node, &edge.to_node)),
            )
        {
            let Some(&source_index) = indices.get(from_node) else {
                return UnknownNodeSnafu {
                    definition_id: from_node.clone(),
                }
                .fail();
            };
            let Some(&target_index) = indices.get(to_node) else {
                return UnknownNodeSnafu {
                    definition_id: to_node.clone(),
                }
                .fail();
            };
            if source_index >= target_index {
                return InvalidEdgeOrderSnafu {
                    from_node: from_node.clone(),
                    to_node: to_node.clone(),
                }
                .fail();
            }
        }

        let (preparations, node_statements) =
            generate_scope(&self.definition, &self.execution_order, "root", &[], None)?;

        let outputs_binding = if self.definition.outputs.is_empty() {
            quote! { let workflow_outputs = mf_runtime::FlowOutputs::new(); }
        } else {
            quote! { let mut workflow_outputs = mf_runtime::FlowOutputs::new(); }
        };
        let mut output_names = BTreeSet::new();
        let mut output_statements: Vec<TokenStream> = Vec::new();
        for output in &self.definition.outputs {
            if !output_names.insert(output.name.as_str()) {
                return DuplicateOutputNameSnafu {
                    name: output.name.clone(),
                }
                .fail();
            }
            if !indices.contains_key(&output.node) {
                return UnknownNodeSnafu {
                    definition_id: output.node.clone(),
                }
                .fail();
            }
            let output_name = LitStr::new(&output.name, Span::call_site());
            let node_id = LitStr::new(output.node.as_str(), Span::call_site());
            let port = LitStr::new(&output.port, Span::call_site());
            let optional = output.optional;
            output_statements.push(quote! {
                if let Some(value) = state.select_output(#output_name, #node_id, #port, #optional)? {
                    workflow_outputs.insert(#output_name.to_owned(), value);
                }
            });
        }

        let generated = quote! {
            pub fn run_workflow(
                registry: &mf_runtime::NodeRegistry,
            ) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
                run_workflow_with_observation(registry, None)
            }

            pub fn run_workflow_with_observation(
                registry: &mf_runtime::NodeRegistry,
                observation: Option<mf_runtime::RunObservation>,
            ) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
                mf_runtime::ExecutionContext::run(observation, |state| run_workflow_in_context(registry, state))
            }

            pub fn run_workflow_in_context(
                registry: &mf_runtime::NodeRegistry,
                state: &mut mf_runtime::ExecutionContext,
            ) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
                #(#preparations)*
                #(#node_statements)*
                #outputs_binding
                #(#output_statements)*
                Ok(workflow_outputs)
            }
        };
        let syntax_tree: syn::File = syn::parse2(generated).context(GeneratedSyntaxSnafu)?;
        let rust_source = prettyplease::unparse(&syntax_tree);

        Ok(GeneratedWorkflowArtifacts {
            rust_source,
            plan_json: self.to_json()?,
        })
    }
}

fn generate_scope(
    definition: &WorkflowDefinition,
    order: &[DefinitionId],
    scope: &str,
    static_scope: &[String],
    enclosing: Option<&LoopDefinition>,
) -> Result<(Vec<TokenStream>, Vec<TokenStream>), PlanError> {
    let nodes_by_id: BTreeMap<_, _> = definition
        .nodes
        .iter()
        .map(|node| (&node.id, node))
        .collect();
    let inference_ident = format_ident!("inference_{scope}");
    let mut preparations = vec![quote! {
        let mut #inference_ident = mf_compiler::TypeInferenceState::default();
    }];
    let mut statements = Vec::new();
    for (index, id) in order.iter().enumerate() {
        let scope_literals: Vec<_> = static_scope
            .iter()
            .map(|id| LitStr::new(id, Span::call_site()))
            .collect();
        let node = nodes_by_id.get(id).ok_or_else(|| PlanError::UnknownNode {
            definition_id: id.clone(),
        })?;
        let node_ident = format_ident!("node_{scope}_{index}");
        let id_lit = LitStr::new(id.as_str(), Span::call_site());
        let kind_lit = LitStr::new(&node.kind, Span::call_site());
        let bindings = dependency_tokens(definition, id);
        if node.kind == ITERATION_KIND {
            preparations.push(iteration_preparation(
                definition,
                node,
                scope,
                index,
                &node_ident,
                &inference_ident,
                &bindings,
            )?);
            statements.push(quote! {
                mf_runtime::execute_node_in_context(&#node_ident, &[#(#bindings),*], state)?;
            });
            continue;
        }

        let constructor = match node.kind.as_str() {
            crate::LOOP_KIND => {
                let loop_definition = node.loop_definition.as_deref().ok_or_else(|| {
                    PlanError::InvalidLoopConfig {
                        definition_id: id.clone(),
                        message: "missing Loop definition".into(),
                    }
                })?;
                let body =
                    crate::loops::body_definition(&loop_definition.body, &definition.dependencies);
                let body_order =
                    crate::compiler::structural_order_graph(&body).map_err(|error| {
                        PlanError::InvalidLoopConfig {
                            definition_id: id.clone(),
                            message: error.to_string(),
                        }
                    })?;
                let child_scope = format!("{scope}_{index}");
                let mut child_static_scope = static_scope.to_vec();
                child_static_scope.push(id.to_string());
                let (body_preparations, body_statements) = generate_scope(
                    &body,
                    &body_order,
                    &child_scope,
                    &child_static_scope,
                    Some(loop_definition),
                )?;
                preparations.extend(body_preparations);
                let config_json = serde_json::to_string(&serde_json::json!({
                    "max_iterations": loop_definition.max_iterations,
                    "variables": loop_definition.variables,
                    "until": loop_definition.until,
                }))
                .context(SerializeSnafu)?;
                let config_lit = LitStr::new(&config_json, Span::call_site());
                quote! {
                    mf_runtime::prepared_loop_node_from_json(#id_lit, #config_lit,
                        move |state: &mut mf_runtime::ExecutionContext| {
                            #(#body_statements)*
                            Ok(())
                        }
                    )?
                }
            }
            crate::LOOP_ASSIGN_KIND => {
                let target = crate::loops::assignment_target(&node.config).map_err(|message| {
                    PlanError::InvalidLoopConfig {
                        definition_id: id.clone(),
                        message,
                    }
                })?;
                let variable = enclosing
                    .and_then(|loop_definition| {
                        loop_definition
                            .variables
                            .iter()
                            .find(|variable| variable.name == target)
                    })
                    .ok_or_else(|| PlanError::InvalidLoopConfig {
                        definition_id: id.clone(),
                        message: format!("unknown assignment variable `{target}`"),
                    })?;
                let target_lit = LitStr::new(&target, Span::call_site());
                let type_json =
                    serde_json::to_string(&variable.value_type).context(SerializeSnafu)?;
                let type_lit = LitStr::new(&type_json, Span::call_site());
                quote! { mf_runtime::prepared_loop_assign_from_json(#id_lit, #target_lit, #type_lit)? }
            }
            crate::EXIT_LOOP_KIND => quote! { mf_runtime::prepared_loop_exit(#id_lit) },
            crate::LOOP_SOURCE_ID => {
                let variables = enclosing.ok_or_else(|| PlanError::InvalidLoopConfig {
                    definition_id: id.clone(),
                    message: "Loop source has no parent Loop".into(),
                })?;
                let variables_json =
                    serde_json::to_string(&variables.variables).context(SerializeSnafu)?;
                let variables_lit = LitStr::new(&variables_json, Span::call_site());
                quote! { mf_runtime::prepared_loop_source_from_json(#variables_lit)? }
            }
            _ => {
                let config_json = serde_json::to_string(&node.config).context(SerializeSnafu)?;
                let config_lit = LitStr::new(&config_json, Span::call_site());
                if static_scope.is_empty() {
                    quote! { state.prepare_node(registry, #id_lit, #kind_lit, #config_lit)? }
                } else {
                    quote! { state.prepare_node_in_loop(
                        registry, #id_lit, #kind_lit, #config_lit, &[#(#scope_literals),*]
                    )? }
                }
            }
        };
        let report_preparation = if static_scope.is_empty() {
            quote! { state.preparation_failed(#id_lit, &error); }
        } else {
            quote! { state.preparation_failed_in_loop(&[#(#scope_literals),*], #id_lit, &error); }
        };
        preparations.push(quote! {
            let mut #node_ident = #constructor;
            #inference_ident.resolve_node(&mut #node_ident, &[#(#bindings),*]).map_err(|error| {
                #report_preparation
                mf_runtime::WorkflowRunError::Context {
                    definition_id: #id_lit.into(),
                    message: error.to_string(),
                }
            })?;
        });
        let exit_check = enclosing.map(|_| {
            quote! {
                if state.loop_exit_requested() {
                    return Ok(());
                }
            }
        });
        statements.push(quote! {
            mf_runtime::execute_node_in_context(&#node_ident, &[#(#bindings),*], state)?;
            #exit_check
        });
    }
    Ok((preparations, statements))
}

fn dependency_tokens(definition: &WorkflowDefinition, id: &DefinitionId) -> Vec<TokenStream> {
    let mut bindings = Vec::new();
    for edge in definition.edges.iter().filter(|edge| &edge.to_node == id) {
        let source = LitStr::new(edge.from_node.as_str(), Span::call_site());
        let port = LitStr::new(&edge.from_output, Span::call_site());
        let input = LitStr::new(&edge.to_input, Span::call_site());
        bindings.push(quote! { mf_runtime::ExecutionDependency { input: Some(#input), source_node: #source, source_output: #port } });
    }
    for edge in definition
        .control_edges
        .iter()
        .filter(|edge| &edge.to_node == id)
    {
        let source = LitStr::new(edge.from_node.as_str(), Span::call_site());
        let port = LitStr::new(&edge.from_output, Span::call_site());
        bindings.push(quote! { mf_runtime::ExecutionDependency { input: None, source_node: #source, source_output: #port } });
    }
    bindings
}

fn iteration_preparation(
    parent: &WorkflowDefinition,
    node: &crate::NodeDefinition,
    scope: &str,
    index: usize,
    node_ident: &syn::Ident,
    inference_ident: &syn::Ident,
    bindings: &[TokenStream],
) -> Result<TokenStream, PlanError> {
    let invalid = |message: String| PlanError::Iteration {
        definition_id: node.id.clone(),
        message,
    };
    let config = parse_config(node).map_err(invalid)?;
    let body = body_definition(parent, &config).map_err(invalid)?;
    let order = crate::structural_order(&body).map_err(|error| invalid(error.to_string()))?;
    let definitions: BTreeMap<_, _> = body.nodes.iter().map(|node| (&node.id, node)).collect();
    let mut preparations = Vec::new();
    let mut executions = Vec::new();
    let mut result_ident = None;
    let body_inference = format_ident!("body_inference_{scope}_{index}");
    let outer_id = LitStr::new(node.id.as_str(), Span::call_site());
    let outer_kind = LitStr::new(ITERATION_KIND, Span::call_site());
    let outer_config = LitStr::new(
        &serde_json::to_string(&node.config).context(SerializeSnafu)?,
        Span::call_site(),
    );
    for (position, id) in order.iter().enumerate() {
        let body_node = definitions[id];
        let body_ident = format_ident!("body_node_{scope}_{index}_{position}");
        if id == &config.body.result.node {
            result_ident = Some(body_ident.clone());
        }
        let inner_id = LitStr::new(id.as_str(), Span::call_site());
        let inner_bindings = dependency_tokens(&body, id);
        let construct = if body_node.kind == ITERATION_INPUT_KIND {
            quote! { mf_runtime::iteration_input_flow_node() }
        } else {
            let kind = LitStr::new(&body_node.kind, Span::call_site());
            let config_json = serde_json::to_string(&body_node.config).context(SerializeSnafu)?;
            let config_lit = LitStr::new(&config_json, Span::call_site());
            quote! {
                mf_runtime::instantiate_node_with_metadata(registry, #inner_id, #kind, #config_lit)
                    .map_err(|error| {
                        state.preparation_failed(#outer_id, &error);
                        mf_runtime::WorkflowRunError::Context {
                            definition_id: #outer_id.into(),
                            message: format!("iteration body: {error}"),
                        }
                    })?
            }
        };
        preparations.push(quote! {
            let mut #body_ident = #construct;
            #body_inference.resolve_node(&mut #body_ident, &[#(#inner_bindings),*])
                .map_err(|error| {
                    state.preparation_failed(#outer_id, &error);
                    mf_runtime::WorkflowRunError::Context {
                        definition_id: #outer_id.into(),
                        message: format!("iteration body: {error}"),
                    }
                })?;
        });
        executions.push(quote! {
            mf_runtime::execute_node_in_context(&#body_ident, &[#(#inner_bindings),*], &mut child)
                .map_err(|source| mf_runtime::NodeExecutionError::PluginFailed {
                    source: Box::new(source),
                })?;
        });
    }
    let result_ident = result_ident.ok_or_else(|| invalid("body result node is unknown".into()))?;
    let result_node = LitStr::new(config.body.result.node.as_str(), Span::call_site());
    let result_port = LitStr::new(&config.body.result.port, Span::call_site());
    let mode = match config.mode {
        IterationMode::Sequential => quote! { mf_runtime::IterationMode::Sequential },
        IterationMode::Parallel => quote! { mf_runtime::IterationMode::Parallel },
    };
    let on_error = match config.on_error {
        IterationErrorPolicy::Terminate => quote! { mf_runtime::IterationErrorPolicy::Terminate },
        IterationErrorPolicy::ContinueOnError => {
            quote! { mf_runtime::IterationErrorPolicy::ContinueOnError }
        }
        IterationErrorPolicy::RemoveFailed => {
            quote! { mf_runtime::IterationErrorPolicy::RemoveFailed }
        }
    };
    let body_nodes = config.body.nodes.iter().map(|body_node| {
        let id = LitStr::new(body_node.id.as_str(), Span::call_site());
        let kind = LitStr::new(&body_node.kind, Span::call_site());
        quote! { mf_runtime::NodeIdentity { id: #id.into(), kind: #kind.into(), path: Vec::new() } }
    });
    Ok(quote! {
        let _registered_iteration = state.prepare_node(registry, #outer_id, #outer_kind, #outer_config)?;
        let mut #body_inference = mf_compiler::TypeInferenceState::default();
        #(#preparations)*
        let result_type = #result_ident.ports.outputs.iter()
            .find(|port| port.name == #result_port)
            .ok_or_else(|| {
                let error = mf_runtime::WorkflowRunError::Context {
                    definition_id: #outer_id.into(),
                    message: format!("iteration body result `{}`.`{}` is unavailable", #result_node, #result_port),
                };
                state.preparation_failed(#outer_id, &error);
                error
            })?
            .value_type.clone();
        let iteration = mf_runtime::IterationNode::new(#outer_id, vec![#(#body_nodes),*], #mode, #on_error, result_type, move |item, index, observation| {
            let mut child = mf_runtime::ExecutionContext::for_iteration_with_observation(item, index, observation)?;
            #(#executions)*
            child.select_output("result", #result_node, #result_port, false)
                .map_err(|source| mf_runtime::NodeExecutionError::PluginFailed {
                    source: Box::new(source),
                })?
                .ok_or_else(|| mf_runtime::NodeExecutionError::ExecutionFailed {
                    message: "iteration body did not produce `result`".into(),
                })
        });
        let ports = iteration.ports();
        let mut #node_ident = mf_runtime::FlowNode::new(#outer_id, Box::new(iteration), ports);
        #inference_ident.resolve_node(&mut #node_ident, &[#(#bindings),*]).map_err(|error| {
            state.preparation_failed(#outer_id, &error);
            mf_runtime::WorkflowRunError::Context {
                definition_id: #outer_id.into(),
                message: error.to_string(),
            }
        })?;
    })
}
