use crate::iteration::parse_config;
use crate::{DefinitionId, NodeRegistry, WorkflowDefinition};
use mf_runtime::{ITERATION_KIND, SCOPE_INPUT_KIND, SubgraphDefinition, ValueType};
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
        self.generate_using_registry(None)
    }

    pub fn generate_artifacts_with_registry(
        &self,
        registry: &NodeRegistry,
    ) -> Result<GeneratedWorkflowArtifacts, PlanError> {
        self.generate_using_registry(Some(registry))
    }

    fn generate_using_registry(
        &self,
        registry: Option<&NodeRegistry>,
    ) -> Result<GeneratedWorkflowArtifacts, PlanError> {
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

        let (preparations, node_statements) = generate_scope(
            &self.definition,
            &self.execution_order,
            GenerationScope {
                name: "root",
                path: &[],
                declaration: None,
                error: None,
                registry,
            },
        )?;

        let mut output_names = BTreeSet::new();
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
        }
        let outputs_return = generate_outputs(&self.definition);

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
                #outputs_return
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

struct GenerationScope<'a> {
    name: &'a str,
    path: &'a [String],
    declaration: Option<&'a SubgraphDefinition>,
    error: Option<&'a TokenStream>,
    registry: Option<&'a NodeRegistry>,
}

fn generate_scope(
    definition: &WorkflowDefinition,
    order: &[DefinitionId],
    context: GenerationScope<'_>,
) -> Result<(Vec<TokenStream>, Vec<TokenStream>), PlanError> {
    let scope = context.name;
    let static_scope = context.path;
    let enclosing = context.declaration;
    let preparation_error_override = context.error;
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
        if let Some(request) = declared_subgraph(node, context.registry)? {
            preparations.push(subgraph_preparation(
                definition, node, &context, index, &bindings, request,
            )?);
            let output_return = generate_outputs(definition);
            let exit_check = enclosing.filter(|scope| scope.allow_state).map(|_| {
                quote! {
                    if state.scope_exit_requested() {
                        return { #output_return };
                    }
                }
            });
            statements.push(quote! {
                mf_runtime::execute_node_in_context(&#node_ident, &[#(#bindings),*], state)?;
                #exit_check
            });
            continue;
        }

        let preparation_error = preparation_error_override.cloned().unwrap_or_else(|| {
            let report = if static_scope.is_empty() {
                quote! { state.preparation_failed(#id_lit, &error); }
            } else {
                quote! { state.preparation_failed_in_loop(&[#(#scope_literals),*], #id_lit, &error); }
            };
            quote! {
                #report
                mf_runtime::WorkflowRunError::Context {
                    definition_id: #id_lit.into(),
                    message: error.to_string(),
                }
            }
        });
        let constructor = match node.kind.as_str() {
            crate::LOOP_ASSIGN_KIND => {
                let target = crate::loops::assignment_target(&node.config).map_err(|message| {
                    PlanError::InvalidLoopConfig {
                        definition_id: id.clone(),
                        message,
                    }
                })?;
                let variable = enclosing
                    .filter(|scope| scope.allow_state)
                    .and_then(|scope| scope.inputs.get(&target))
                    .ok_or_else(|| PlanError::InvalidLoopConfig {
                        definition_id: id.clone(),
                        message: format!("unknown assignment variable {target}"),
                    })?;
                let target_lit = LitStr::new(&target, Span::call_site());
                let type_json =
                    serde_json::to_string(&variable.descriptor()).context(SerializeSnafu)?;
                let type_lit = LitStr::new(&type_json, Span::call_site());
                quote! { mf_runtime::prepared_loop_assign_from_json(#id_lit, #target_lit, #type_lit)? }
            }
            crate::EXIT_LOOP_KIND => quote! { mf_runtime::prepared_loop_exit(#id_lit) },
            SCOPE_INPUT_KIND => {
                let request = enclosing.ok_or_else(|| PlanError::InvalidLoopConfig {
                    definition_id: id.clone(),
                    message: "scope source has no declaration".into(),
                })?;
                let inputs: BTreeMap<_, _> = request
                    .inputs
                    .iter()
                    .map(|(name, value_type)| (name, value_type.descriptor()))
                    .collect();
                let types = LitStr::new(
                    &serde_json::to_string(&inputs).context(SerializeSnafu)?,
                    Span::call_site(),
                );
                quote! { mf_runtime::prepared_scope_source_from_json(#id_lit, #types)? }
            }
            _ => {
                let config_json = serde_json::to_string(&node.config).context(SerializeSnafu)?;
                let config_lit = LitStr::new(&config_json, Span::call_site());
                if preparation_error_override.is_some() {
                    quote! {
                        mf_runtime::instantiate_node_with_metadata(registry, #id_lit, #kind_lit, #config_lit)
                            .map_err(|error| { #preparation_error })?
                    }
                } else if static_scope.is_empty() {
                    quote! { state.prepare_node(registry, #id_lit, #kind_lit, #config_lit)? }
                } else {
                    quote! { state.prepare_node_in_loop(
                        registry, #id_lit, #kind_lit, #config_lit, &[#(#scope_literals),*]
                    )? }
                }
            }
        };
        preparations.push(quote! {
            let mut #node_ident = #constructor;
            #inference_ident.resolve_node(&mut #node_ident, &[#(#bindings),*]).map_err(|error| {
                #preparation_error
            })?;
        });
        let output_return = generate_outputs(definition);
        let exit_check = enclosing.filter(|scope| scope.allow_state).map(|_| {
            quote! {
                if state.scope_exit_requested() {
                    return { #output_return };
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

fn subgraph_preparation(
    parent: &WorkflowDefinition,
    node: &crate::NodeDefinition,
    context: &GenerationScope<'_>,
    index: usize,
    bindings: &[TokenStream],
    request: SubgraphDefinition,
) -> Result<TokenStream, PlanError> {
    let scope = context.name;
    let static_scope = context.path;
    let node_ident = format_ident!("node_{scope}_{index}");
    let inference_ident = format_ident!("inference_{scope}");
    let declaration_ident = format_ident!("declaration_{scope}_{index}");
    let body_ident = format_ident!("body_{scope}_{index}");
    let outer_id = LitStr::new(node.id.as_str(), Span::call_site());
    let kind = LitStr::new(&node.kind, Span::call_site());
    let config = LitStr::new(
        &serde_json::to_string(&node.config).context(SerializeSnafu)?,
        Span::call_site(),
    );
    let body = request.workflow(parent);
    let mut body_static_scope = static_scope.to_vec();
    let preparation_error = if request.allow_state {
        body_static_scope.push(node.id.to_string());
        None
    } else {
        body_static_scope.clear();
        Some(quote! {
            state.preparation_failed(#outer_id, &error);
            mf_runtime::WorkflowRunError::Context {
                definition_id: #outer_id.into(), message: format!("subgraph body: {error}"),
            }
        })
    };
    let order = crate::compiler::structural_order_graph(&body).map_err(|error| {
        if node.kind == crate::LOOP_KIND {
            PlanError::InvalidLoopConfig {
                definition_id: node.id.clone(),
                message: error.to_string(),
            }
        } else {
            PlanError::Iteration {
                definition_id: node.id.clone(),
                message: error.to_string(),
            }
        }
    })?;
    let body_scope = format!("{scope}_{index}");
    let (preparations, executions) = generate_scope(
        &body,
        &order,
        GenerationScope {
            name: &body_scope,
            path: &body_static_scope,
            declaration: Some(&request),
            error: preparation_error.as_ref(),
            registry: context.registry,
        },
    )?;
    let nodes = body.nodes.iter()
        .filter(|node| node.kind != SCOPE_INPUT_KIND)
        .map(|node| {
            let id = LitStr::new(node.id.as_str(), Span::call_site());
            let kind = LitStr::new(&node.kind, Span::call_site());
            quote! { mf_runtime::NodeIdentity { id: #id.into(), kind: #kind.into(), path: Vec::new() } }
        });
    let outputs = body.outputs.iter().map(|output| {
        let position = order
            .iter()
            .position(|id| id == &output.node)
            .expect("validated body output");
        let result = format_ident!("node_{body_scope}_{position}");
        let name = LitStr::new(&output.name, Span::call_site());
        let port = LitStr::new(&output.port, Span::call_site());
        let source = LitStr::new(output.node.as_str(), Span::call_site());
        let required = !output.optional;
        quote! {
            mf_runtime::PortSpec::owned(#name, #result.ports.outputs.iter()
                .find(|port| port.name == #port)
                .ok_or_else(|| {
                    let error = mf_runtime::WorkflowRunError::Context {
                        definition_id: #outer_id.into(),
                        message: format!("body output `{}`.`{}` is unavailable", #source, #port),
                    };
                    state.preparation_failed(#outer_id, &error);
                    error
                })?.value_type.clone(), #required)
        }
    });
    let body_outputs = generate_outputs(&body);
    let options = LitStr::new(
        &serde_json::to_string(&request.options).context(SerializeSnafu)?,
        Span::call_site(),
    );
    let scopes: Vec<_> = static_scope
        .iter()
        .map(|id| LitStr::new(id, Span::call_site()))
        .collect();
    let declaration = if static_scope.is_empty() {
        quote! { state.prepare_node(registry, #outer_id, #kind, #config)? }
    } else {
        quote! { state.prepare_node_in_loop(registry, #outer_id, #kind, #config, &[#(#scopes),*])? }
    };
    let report = if static_scope.is_empty() {
        quote! { state.preparation_failed(#outer_id, &error); }
    } else {
        quote! { state.preparation_failed_in_loop(&[#(#scopes),*], #outer_id, &error); }
    };
    Ok(quote! {
        let #declaration_ident = #declaration;
        #(#preparations)*
        let #body_ident = mf_runtime::PreparedSubgraph::new(
            vec![#(#nodes),*], vec![#(#outputs),*],
            move |state| {
                #(#executions)*
                #body_outputs
            },
        );
        let mut #node_ident = #declaration_ident.with_subgraph_from_json(#options, #body_ident)
            .map_err(|error| { #report error })?;
        #inference_ident.resolve_node(&mut #node_ident, &[#(#bindings),*])
            .map_err(|error| {
                #report
                mf_runtime::WorkflowRunError::Context {
                    definition_id: #outer_id.into(), message: error.to_string(),
                }
            })?;
    })
}

fn generate_outputs(definition: &WorkflowDefinition) -> TokenStream {
    let binding = if definition.outputs.is_empty() {
        quote! { let workflow_outputs = mf_runtime::FlowOutputs::new(); }
    } else {
        quote! { let mut workflow_outputs = mf_runtime::FlowOutputs::new(); }
    };
    let outputs = definition.outputs.iter().map(|output| {
        let name = LitStr::new(&output.name, Span::call_site());
        let node = LitStr::new(output.node.as_str(), Span::call_site());
        let port = LitStr::new(&output.port, Span::call_site());
        let optional = output.optional;
        quote! {
            if let Some(value) = state.select_output(#name, #node, #port, #optional)? {
                workflow_outputs.insert(#name.to_owned(), value);
            }
        }
    });
    quote! { #binding #(#outputs)* Ok(workflow_outputs) }
}

fn declared_subgraph(
    node: &crate::NodeDefinition,
    registry: Option<&NodeRegistry>,
) -> Result<Option<SubgraphDefinition>, PlanError> {
    let invalid = |message| PlanError::Iteration {
        definition_id: node.id.clone(),
        message,
    };
    if matches!(
        node.kind.as_str(),
        SCOPE_INPUT_KIND | crate::LOOP_ASSIGN_KIND | crate::EXIT_LOOP_KIND
    ) {
        return Ok(None);
    }
    if let Some(registry) = registry {
        let registration = registry
            .get(&node.kind)
            .ok_or_else(|| invalid(format!("unknown kind {}", node.kind)))?;
        let declaration = registration
            .instantiate(node.config.clone())
            .map_err(|error| invalid(error.to_string()))?;
        return declaration
            .subgraph_definition(node)
            .map_err(|error| invalid(error.to_string()));
    }
    if let Some(definition) = node.loop_definition.as_deref() {
        return Ok(Some(SubgraphDefinition {
            body: definition.body.clone(),
            body_pointer: "/loop/body".into(),
            source_id: crate::LOOP_SOURCE_ID.into(),
            inputs: mf_runtime::loop_variable_types(&definition.variables).map_err(invalid)?,
            outputs: Vec::new(),
            allow_state: true,
            options: serde_json::json!({
                "max_iterations": definition.max_iterations,
                "variables": definition.variables, "until": definition.until,
            }),
        }));
    }
    if node.kind == ITERATION_KIND {
        let config = parse_config(node).map_err(invalid)?;
        return Ok(Some(SubgraphDefinition {
            body: mf_runtime::LoopBodyDefinition {
                nodes: config.body.nodes,
                edges: config.body.edges,
                control_edges: config.body.control_edges,
            },
            body_pointer: "/config/body".into(),
            source_id: mf_runtime::ITERATION_INPUT_ID.into(),
            inputs: BTreeMap::from([("items".into(), ValueType::Any)]),
            outputs: vec![mf_runtime::WorkflowOutputDefinition {
                name: "result".into(),
                node: config.body.result.node,
                port: config.body.result.port,
                optional: false,
            }],
            options: serde_json::Value::Null,
            allow_state: false,
        }));
    }
    Ok(None)
}
