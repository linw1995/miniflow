#[cfg(feature = "codegen")]
use crate::LoopDefinition;
#[cfg(feature = "codegen")]
use crate::iteration::{body_definition, parse_config};
use crate::{DefinitionId, WorkflowDefinition};
#[cfg(feature = "codegen")]
use mf_runtime::{ITERATION_INPUT_KIND, ITERATION_KIND};
#[cfg(feature = "codegen")]
use proc_macro2::{Span, TokenStream};
#[cfg(feature = "codegen")]
use quote::{format_ident, quote};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::collections::BTreeMap;
#[cfg(feature = "codegen")]
use std::collections::BTreeSet;
#[cfg(feature = "codegen")]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedExecutionArtifacts {
    pub rust_source: String,
    pub manifest_bytes: Vec<u8>,
}

#[derive(Debug, Snafu)]
pub enum PlanError {
    #[snafu(transparent)]
    Manifest {
        source: mf_runtime::WorkflowManifestError,
    },
    #[snafu(transparent)]
    Description { source: crate::DescriptionError },
    #[snafu(transparent)]
    Compilation {
        #[snafu(source(from(crate::WorkflowCompileError, Box::new)))]
        source: Box<crate::WorkflowCompileError>,
    },
    #[snafu(transparent)]
    Construction { source: crate::WorkflowBuildError },
    #[snafu(display("invalid streaming plan: {source}"))]
    Stream {
        #[snafu(source(from(crate::WorkflowCompileError, Box::new)))]
        source: Box<crate::WorkflowCompileError>,
    },
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
    #[cfg(feature = "codegen")]
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
    pub fn start_stream_observation(
        &self,
        observer: &mf_telemetry::observation::Observer,
        run_id: mf_telemetry::identity::RunId,
    ) -> Result<mf_runtime::StreamObservation, crate::DescriptionError> {
        let description = crate::describe_compiled(self)?;
        observer
            .start_stream(description, run_id)
            .context(crate::compiler::ContractSnafu)
    }

    pub fn start_observation(
        &self,
        observer: &mf_telemetry::observation::Observer,
        run_id: mf_telemetry::identity::RunId,
    ) -> Result<mf_runtime::RunObservation, crate::DescriptionError> {
        ensure!(
            self.definition.execution.is_none(),
            crate::compiler::InvalidObservationSnafu {
                message: "use start_stream_observation for streaming workflows",
            }
        );
        if self.definition.version != mf_runtime::WorkflowDefinitionVersion::V2026_09_26 {
            let description = crate::describe_compiled(self)?;
            return observer
                .start_with_description(description, run_id)
                .context(crate::compiler::ContractSnafu);
        }
        ensure!(
            self.execution_order.len() == self.definition.nodes.len(),
            crate::compiler::InvalidObservationSnafu {
                message: "observation order is incomplete",
            }
        );
        let nodes_by_id: BTreeMap<_, _> = self
            .definition
            .nodes
            .iter()
            .map(|node| (&node.id, node))
            .collect();
        ensure!(
            nodes_by_id.len() == self.definition.nodes.len(),
            crate::compiler::InvalidObservationSnafu {
                message: "duplicate observation node ID",
            }
        );
        let order: Vec<String> = self
            .execution_order
            .iter()
            .map(ToString::to_string)
            .collect();
        let id = mf_telemetry::identity::WorkflowId::from_definition(&self.definition, &order)
            .context(crate::compiler::ContractSnafu)?;
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
                    .with_context(|| crate::compiler::InvalidObservationSnafu {
                        message: format!("unknown observation node {id}"),
                    })
            })
            .collect::<Result<_, _>>()?;
        observer
            .start(id, run_id, nodes)
            .context(crate::compiler::ContractSnafu)
    }

    pub fn to_json(&self) -> Result<String, PlanError> {
        serde_json::to_string(self).context(SerializeSnafu)
    }

    pub fn from_json(input: &str) -> Result<Self, PlanError> {
        let plan: Self = serde_json::from_str(input).context(ParseSnafu)?;
        crate::validate_execution(&plan.definition).map_err(crate::WorkflowBuildError::from)?;
        Ok(plan)
    }

    #[cfg(feature = "codegen")]
    pub fn generate_artifacts(&self) -> Result<GeneratedWorkflowArtifacts, PlanError> {
        if self.definition.execution.is_some() {
            return generate_stream_artifacts(self);
        }
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

        let (preparations, _, root_flow) = generate_scope(
            &self.definition,
            &self.execution_order,
            "root",
            &[],
            None,
            None,
            true,
        )?;

        let mut output_names = BTreeSet::new();
        for output in &self.definition.outputs {
            if !output_names.insert(output.name.as_ref()) {
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
        let generated = quote! {
            include!(concat!(env!("OUT_DIR"), "/flow-plans.rs"));

            pub fn prepare_workflow(
                registry: &mf_runtime::NodeRegistry,
                mut observation: Option<&mut mf_runtime::RunObservation>,
            ) -> Result<mf_runtime::Flow, mf_compiler::WorkflowBuildError> {
                use snafu::ResultExt as _;
                #(#preparations)*
                Ok(#root_flow)
            }

            pub fn run_workflow_with_inputs(flow: &mf_runtime::Flow, arguments: mf_runtime::WorkflowArguments) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
                run_workflow_with_inputs_and_options(flow, arguments, mf_runtime::RuntimeOptions::default())
            }

            pub fn run_workflow_with_inputs_and_options(
                flow: &mf_runtime::Flow,
                arguments: mf_runtime::WorkflowArguments,
                options: mf_runtime::RuntimeOptions,
            ) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
                let mut state = mf_runtime::ExecutionContext::default();
                state.set_workflow_arguments(arguments);
                run_workflow_in_context_with_options(flow, &mut state, options)
            }

            pub fn run_workflow(
                flow: &mf_runtime::Flow,
            ) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
                run_workflow_with_observation(flow, None)
            }

            pub fn run_workflow_with_observation(
                flow: &mf_runtime::Flow,
                observation: Option<mf_runtime::RunObservation>,
            ) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
                run_workflow_with_observation_and_options(flow, observation, mf_runtime::RuntimeOptions::default())
            }

            pub fn run_workflow_with_observation_and_options(
                flow: &mf_runtime::Flow,
                observation: Option<mf_runtime::RunObservation>,
                options: mf_runtime::RuntimeOptions,
            ) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
                mf_runtime::ExecutionContext::run(observation, |state| {
                    run_workflow_in_context_with_options(flow, state, options)
                })
            }

            pub fn run_workflow_in_context(
                flow: &mf_runtime::Flow,
                state: &mut mf_runtime::ExecutionContext,
            ) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
                run_workflow_in_context_with_options(flow, state, mf_runtime::RuntimeOptions::default())
            }

            pub fn run_workflow_in_context_with_options(
                flow: &mf_runtime::Flow,
                state: &mut mf_runtime::ExecutionContext,
                options: mf_runtime::RuntimeOptions,
            ) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
                mf_runtime::FlowRuntime::new(options).execute_in_context(flow, state)
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

#[cfg(feature = "codegen")]
fn generate_stream_artifacts(
    plan: &CompiledWorkflow,
) -> Result<GeneratedWorkflowArtifacts, PlanError> {
    if crate::structural_order(&plan.definition).context(StreamSnafu)? != plan.execution_order {
        return InvalidExecutionOrderSnafu.fail();
    }
    let definition = &plan.definition;
    let order = &plan.execution_order;
    let (preparations, _, _) = generate_scope(definition, order, "stream", &[], None, None, false)?;
    let nodes = (0..order.len()).map(|index| format_ident!("node_stream_{index}"));
    let execution = LitStr::new(
        &serde_json::to_string(definition.execution.as_ref().unwrap()).context(SerializeSnafu)?,
        Span::call_site(),
    );
    let generated = quote! {
        include!(concat!(env!("OUT_DIR"), "/flow-plans.rs"));

        pub fn prepare_stream(registry: &mf_runtime::NodeRegistry) -> Result<mf_runtime::PreparedStream, mf_compiler::WorkflowBuildError> {
            use snafu::ResultExt as _;
            let mut observation: Option<&mut mf_runtime::RunObservation> = None;
            #(#preparations)*
            let execution = serde_json::from_str::<mf_runtime::StreamExecution>(#execution)
                .boxed()
                .context(mf_compiler::WorkflowMetadataSnafu { definition_id: "<workflow>" })?;
            let nodes = vec![#(#nodes),*];
            let input_schema = mf_compiler::stream_workflow_inputs(&nodes, STREAM_DEPENDENCIES)?;
            Ok(mf_runtime::PreparedStream::from_plan(
                execution, nodes, std::borrow::Cow::Borrowed(STREAM_DEPENDENCIES),
                STREAM_MESSAGE_DOMAINS.clone(), std::borrow::Cow::Borrowed(STREAM_OUTPUTS), input_schema,
            ))
        }
    };
    let syntax: syn::File = syn::parse2(generated).context(GeneratedSyntaxSnafu)?;
    Ok(GeneratedWorkflowArtifacts {
        rust_source: prettyplease::unparse(&syntax),
        plan_json: plan.to_json()?,
    })
}

#[cfg(feature = "codegen")]
fn generate_scope(
    definition: &WorkflowDefinition,
    order: &[DefinitionId],
    scope: &str,
    static_scope: &[String],
    enclosing: Option<&LoopDefinition>,
    preparation_error_override: Option<&TokenStream>,
    build_flow: bool,
) -> Result<(Vec<TokenStream>, Vec<TokenStream>, syn::Ident), PlanError> {
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
    let mut node_idents = Vec::with_capacity(order.len());
    let incoming = crate::compiler::incoming_dependencies(definition);
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
        let dependencies = incoming.get(id.as_str()).map_or(&[][..], Vec::as_slice);
        let bindings = dependency_tokens(dependencies);
        if matches!(node.kind.as_str(), crate::LOOP_KIND | ITERATION_KIND) {
            preparations.push(subgraph_preparation(
                definition,
                node,
                scope,
                index,
                static_scope,
                &bindings,
            )?);
            node_idents.push(node_ident);
            continue;
        }

        let preparation_report = preparation_error_override.cloned().unwrap_or_else(|| {
            if static_scope.is_empty() {
                quote! {
                    if let Some(observation) = observation.as_deref_mut() {
                        observation.preparation_failed(#id_lit, error.to_string());
                    }
                }
            } else {
                quote! {
                    if let Some(observation) = observation.as_deref_mut() {
                        observation.preparation_failed_unattributed(format!(
                            "Loop scope {} node `{}`: {}",
                            serde_json::to_string(&[#(#scope_literals),*]).expect("scope IDs serialize"),
                            #id_lit, error,
                        ));
                    }
                }
            }
        });
        let constructor = match node.kind.as_str() {
            crate::LOOP_ASSIGN_KIND => {
                let mut path: Vec<_> = static_scope
                    .iter()
                    .map(|id| DefinitionId::from(id.as_str()))
                    .collect();
                path.push(id.clone());
                let target = crate::loops::assignment_target(&node.config, &path)?;
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
                quote! { mf_compiler::prepared_loop_assign_from_json(#id_lit, #target_lit, #type_lit)? }
            }
            crate::EXIT_LOOP_KIND => quote! { mf_runtime::prepared_loop_exit(#id_lit) },
            ITERATION_INPUT_KIND => quote! { mf_runtime::iteration_input_flow_node() },
            crate::LOOP_SOURCE_ID => {
                let variables = enclosing.ok_or_else(|| PlanError::InvalidLoopConfig {
                    definition_id: id.clone(),
                    message: "Loop source has no parent Loop".into(),
                })?;
                let variables_json =
                    serde_json::to_string(&variables.variables).context(SerializeSnafu)?;
                let variables_lit = LitStr::new(&variables_json, Span::call_site());
                quote! { mf_compiler::prepared_loop_source_from_json(#variables_lit)? }
            }
            _ => {
                let config_json = serde_json::to_string(&node.config).context(SerializeSnafu)?;
                let config_lit = LitStr::new(&config_json, Span::call_site());
                quote! {
                    mf_compiler::instantiate_node_with_metadata(registry, #id_lit, #kind_lit, #config_lit)
                        .inspect_err(|error| { #preparation_report })?
                }
            }
        };
        preparations.push(quote! {
            let mut #node_ident = #constructor;
            #inference_ident.resolve_node(&mut #node_ident, &[#(#bindings),*])
                .inspect_err(|error| { #preparation_report })
                .boxed()
                .context(mf_compiler::WorkflowMetadataSnafu { definition_id: #id_lit })?;
        });
        node_idents.push(node_ident);
    }
    preparations.push(quote! { drop(#inference_ident); });
    let flow_ident = format_ident!("flow_{scope}");
    if build_flow {
        let nodes = node_idents.iter();
        let plan_ident = format_ident!("FLOW_PLAN_{}", scope.to_uppercase());
        let bind_inputs = scope == "root" && definition.version.supports_startup_inputs();
        let flow_binding = if bind_inputs {
            quote! { let mut #flow_ident = mf_runtime::Flow::from_plan(
                vec![#(mf_compiler::into_task(#nodes)?),*], #plan_ident.clone(), Default::default(),
            ); }
        } else {
            quote! { let #flow_ident = mf_runtime::Flow::from_plan(
                vec![#(mf_compiler::into_task(#nodes)?),*], #plan_ident.clone(), Default::default(),
            ); }
        };
        preparations.push(flow_binding);
        if bind_inputs {
            preparations
                .push(quote! { #flow_ident = mf_compiler::bind_workflow_inputs(#flow_ident)?; });
        }
        statements.push(quote! { runtime.execute_in_context(&#flow_ident, state) });
    }
    Ok((preparations, statements, flow_ident))
}

#[cfg(feature = "codegen")]
fn dependency_tokens(dependencies: &[mf_runtime::ExecutionDependency<'_>]) -> Vec<TokenStream> {
    dependencies.iter().map(|dependency| {
        let source = LitStr::new(dependency.source_node, Span::call_site());
        let port = LitStr::new(dependency.source_output, Span::call_site());
        let input = match dependency.input {
            Some(input) => {
                let input = LitStr::new(input, Span::call_site());
                quote! { Some(#input) }
            }
            None => quote! { None },
        };
        quote! { mf_runtime::ExecutionDependency { input: #input, source_node: #source, source_output: #port } }
    }).collect()
}

#[cfg(feature = "codegen")]
fn subgraph_preparation(
    parent: &WorkflowDefinition,
    node: &crate::NodeDefinition,
    scope: &str,
    index: usize,
    static_scope: &[String],
    bindings: &[TokenStream],
) -> Result<TokenStream, PlanError> {
    let node_ident = format_ident!("node_{scope}_{index}");
    let inference_ident = format_ident!("inference_{scope}");
    let body_ident = format_ident!("body_{scope}_{index}");
    let outer_id = LitStr::new(node.id.as_str(), Span::call_site());
    let kind = LitStr::new(&node.kind, Span::call_site());
    let config = LitStr::new(
        &serde_json::to_string(&node.config).context(SerializeSnafu)?,
        Span::call_site(),
    );
    let mut body_static_scope = static_scope.to_vec();
    let (body, options, enclosing, preparation_error) = if node.kind == crate::LOOP_KIND {
        let definition =
            node.loop_definition
                .as_deref()
                .ok_or_else(|| PlanError::InvalidLoopConfig {
                    definition_id: node.id.clone(),
                    message: "missing Loop definition".into(),
                })?;
        body_static_scope.push(node.id.to_string());
        (
            crate::loops::body_definition(&definition.body, &parent.dependencies),
            serde_json::json!({
                "max_iterations": definition.max_iterations,
                "variables": definition.variables,
                "until": definition.until,
            }),
            Some(definition),
            None,
        )
    } else {
        let invalid = |message| PlanError::Iteration {
            definition_id: node.id.clone(),
            message,
        };
        let config = parse_config(node)?;
        let body = body_definition(parent, &config).map_err(invalid)?;
        body_static_scope.clear();
        let error = quote! {
            if let Some(observation) = observation.as_deref_mut() {
                observation.preparation_failed(#outer_id, error.to_string());
            }
        };
        (body, serde_json::Value::Null, None, Some(error))
    };
    let order = crate::compiler::structural_order_graph(&body);
    let order = if node.kind == crate::LOOP_KIND {
        order.context(crate::compiler::LoopBodySnafu {
            path: format!("{:?}", node.id),
        })?
    } else {
        order.context(crate::compiler::IterationBodySnafu {
            definition_id: node.id.clone(),
        })?
    };
    let body_scope = format!("{scope}_{index}");
    let (preparations, execution, body_flow) = generate_scope(
        &body,
        &order,
        &body_scope,
        &body_static_scope,
        enclosing,
        preparation_error.as_ref(),
        true,
    )?;
    let nodes = body.nodes.iter()
        .filter(|node| node.kind != ITERATION_INPUT_KIND && node.kind != crate::LOOP_SOURCE_ID)
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
        let position_lit = syn::Index::from(position);
        let name = LitStr::new(&output.name, Span::call_site());
        let port = LitStr::new(&output.port, Span::call_site());
        let source = LitStr::new(output.node.as_str(), Span::call_site());
        let required = !output.optional;
        quote! {
            mf_runtime::PortSpec::owned(#name, #body_flow.node_metadata(&mf_runtime::NodeId::new(#position_lit))
                .into_iter().flat_map(|metadata| metadata.ports.outputs.iter())
                .find(|port| port.name == #port)
                .ok_or_else(|| {
                    mf_compiler::WorkflowInvalidDefinitionSnafu {
                        definition_id: #outer_id,
                        message: format!("body output `{}`.`{}` is unavailable", #source, #port),
                    }.build()
                })?.value_type.clone(), #required)
        }
    });
    let options = LitStr::new(
        &serde_json::to_string(&options).context(SerializeSnafu)?,
        Span::call_site(),
    );
    let scopes: Vec<_> = static_scope
        .iter()
        .map(|id| LitStr::new(id, Span::call_site()))
        .collect();
    let report = if static_scope.is_empty() {
        quote! {
            if let Some(observation) = observation.as_deref_mut() {
                observation.preparation_failed(#outer_id, error.to_string());
            }
        }
    } else {
        quote! {
            if let Some(observation) = observation.as_deref_mut() {
                observation.preparation_failed_unattributed(format!(
                    "Loop scope {} node `{}`: {}",
                    serde_json::to_string(&[#(#scopes),*]).expect("scope IDs serialize"),
                    #outer_id, error,
                ));
            }
        }
    };
    Ok(quote! {
        let #body_flow = (|| -> Result<mf_runtime::Flow, mf_compiler::WorkflowBuildError> {
            #(#preparations)*
            Ok(#body_flow)
        })().context(mf_compiler::WorkflowSubgraphSnafu { definition_id: #outer_id })?;
        let #body_ident = mf_runtime::PreparedSubgraph::new(
            vec![#(#nodes),*], vec![#(#outputs),*],
            move |state| {
                let runtime = mf_runtime::FlowRuntime::default();
                #(#execution)*
            },
        );
        let mut #node_ident = mf_compiler::instantiate_subgraph_with_metadata(
            registry, #outer_id, #kind, #config, #options, #body_ident,
        )
            .inspect_err(|error| { #report })?;
        #inference_ident.resolve_node(&mut #node_ident, &[#(#bindings),*])
            .inspect_err(|error| { #report })
            .boxed()
            .context(mf_compiler::WorkflowMetadataSnafu { definition_id: #outer_id })?;
    })
}

#[cfg(feature = "codegen")]
impl CompiledWorkflow {
    /// Emits immutable layouts while linked providers are available to the generated Cargo build.
    pub fn generate_execution_plans(
        &self,
        registry: &mf_runtime::NodeRegistry,
    ) -> Result<GeneratedExecutionArtifacts, PlanError> {
        let (nodes, order) = crate::compiler::prepare_definition(&self.definition, registry)?;
        if order != self.execution_order {
            return InvalidExecutionOrderSnafu.fail();
        }
        let flow = crate::FlowBuilder::prepare(
            nodes,
            self.definition.edges.clone(),
            order.clone(),
            self.definition.outputs.clone(),
        )
        .and_then(|flow| flow.with_control_edges(self.definition.control_edges.clone()))
        .map_err(crate::WorkflowBuildError::from)?;
        let flow = if self.definition.version.supports_startup_inputs() {
            flow.with_workflow_inputs()
                .map_err(crate::WorkflowBuildError::from)?
        } else {
            flow
        };
        let schema = flow.input_schema().clone();
        let mut layouts = Vec::new();
        if let Some(execution) = &self.definition.execution {
            let prepared = flow
                .into_stream(execution.clone())
                .map_err(crate::WorkflowBuildError::from)?;
            let message = prepared.plan().message_domains();
            let sources = message.sources().iter().map(|source| match source {
                Some(source) => quote! { Some(#source) },
                None => quote! { None },
            });
            let outputs = self.definition.outputs.iter().map(output_definition_tokens);
            let output_domains = (0..order.len()).map(|index| message.output_domain(index));
            let selected = match message.selected_domain() {
                Some(selected) => quote! { Some(#selected) },
                None => quote! { None },
            };
            let executions = domain_tokens(message.execution_domains());
            let by_message = (0..message.sources().len()).map(|id| {
                let values = message.execution_domains_for_message(id);
                quote! { std::borrow::Cow::Borrowed(&[#(#values),*]) }
            });
            let by_execution = (0..message.execution_domains().len())
                .map(|id| message.message_domain_for_execution(id));
            let dependencies = dependency_layout_tokens(&self.definition, &order);
            layouts.push(quote! {
                pub static STREAM_DEPENDENCIES: &[std::borrow::Cow<'static, [mf_runtime::FlowDependency]>] = &[#(#dependencies),*];
                pub static STREAM_OUTPUTS: &[mf_runtime::WorkflowOutputDefinition] = &[#(#outputs),*];
                pub static STREAM_MESSAGE_DOMAINS: mf_runtime::MessageDomains = mf_runtime::MessageDomains::from_parts(
                    std::borrow::Cow::Borrowed(&[#(#sources),*]),
                    std::borrow::Cow::Borrowed(&[#(#output_domains),*]),
                    #selected, #executions,
                    std::borrow::Cow::Borrowed(&[#(#by_message),*]),
                    std::borrow::Cow::Borrowed(&[#(#by_execution),*]),
                );
            });
            collect_body_layouts(&self.definition, &order, "stream", &mut layouts)?;
        } else {
            let flow = flow.into_tasks().map_err(crate::WorkflowBuildError::from)?;
            layouts.push(task_layout_tokens(
                &self.definition,
                &order,
                "root",
                flow.execution_domains(),
            ));
            collect_body_layouts(&self.definition, &order, "root", &mut layouts)?;
        }
        let syntax: syn::File =
            syn::parse2(quote! { #(#layouts)* }).context(GeneratedSyntaxSnafu)?;
        let description = crate::describe_compiled(self)?;
        let interface = mf_runtime::WorkflowInterface {
            version: mf_runtime::WorkflowInterfaceVersion::V2026_10_03,
            workflow_id: description.workflow_id.clone(),
            schema,
        };
        let manifest = mf_runtime::WorkflowManifest {
            version: mf_runtime::WorkflowManifestVersion::V2026_10_07,
            description,
            interface,
        };
        Ok(GeneratedExecutionArtifacts {
            rust_source: prettyplease::unparse(&syntax),
            manifest_bytes: manifest.to_bytes()?,
        })
    }
}

#[cfg(feature = "codegen")]
fn dependency_layout_tokens(
    definition: &WorkflowDefinition,
    order: &[DefinitionId],
) -> Vec<TokenStream> {
    let incoming = crate::compiler::incoming_dependencies(definition);
    order
        .iter()
        .map(|id| {
            let mut dependencies = incoming.get(id.as_str()).cloned().unwrap_or_default();
            dependencies.sort();
            let values = dependencies.iter().map(|dependency| {
                let source = LitStr::new(dependency.source_node, Span::call_site());
                let output = LitStr::new(dependency.source_output, Span::call_site());
                let input = match dependency.input {
                    Some(input) => {
                        let input = LitStr::new(input, Span::call_site());
                        quote! { Some(std::borrow::Cow::Borrowed(#input)) }
                    }
                    None => quote! { None },
                };
                quote! { mf_runtime::FlowDependency {
                    input: #input,
                    source_node: std::borrow::Cow::Borrowed(#source),
                    source_output: std::borrow::Cow::Borrowed(#output),
                } }
            });
            quote! { std::borrow::Cow::Borrowed(&[#(#values),*]) }
        })
        .collect()
}

#[cfg(feature = "codegen")]
fn domain_tokens(plan: &mf_runtime::ExecutionDomains) -> TokenStream {
    let domains = plan.domains().iter().map(|domain| {
        let id = domain.id;
        let first = domain.first_position;
        let nodes = domain.positions.iter().map(|index| {
            quote! { mf_runtime::NodeId::new(#index) }
        });
        let positions = domain.positions.iter();
        let predecessors = domain.predecessors.iter();
        let successors = domain.successors.iter();
        quote! { mf_runtime::ExecutionDomain {
            id: #id, first_position: #first,
            nodes: std::borrow::Cow::Borrowed(&[#(#nodes),*]),
            positions: std::borrow::Cow::Borrowed(&[#(#positions),*]),
            predecessors: std::borrow::Cow::Borrowed(&[#(#predecessors),*]),
            successors: std::borrow::Cow::Borrowed(&[#(#successors),*]),
        } }
    });
    let ancestors = (0..plan.len()).map(|id| {
        let values = plan.ancestor_domains(id);
        quote! { std::borrow::Cow::Borrowed(&[#(#values),*]) }
    });
    quote! { mf_runtime::ExecutionDomains::from_parts(
        std::borrow::Cow::Borrowed(&[#(#domains),*]),
        std::borrow::Cow::Borrowed(&[#(#ancestors),*]),
    ) }
}

#[cfg(feature = "codegen")]
fn output_definition_tokens(output: &mf_runtime::WorkflowOutputDefinition) -> TokenStream {
    let name = LitStr::new(&output.name, Span::call_site());
    let node = LitStr::new(output.node.as_str(), Span::call_site());
    let port = LitStr::new(&output.port, Span::call_site());
    let optional = output.optional;
    quote! { mf_runtime::WorkflowOutputDefinition {
        name: std::borrow::Cow::Borrowed(#name),
        node: mf_runtime::DefinitionId::from_static(#node),
        port: std::borrow::Cow::Borrowed(#port), optional: #optional,
    } }
}

#[cfg(feature = "codegen")]
fn task_layout_tokens(
    definition: &WorkflowDefinition,
    order: &[DefinitionId],
    scope: &str,
    domains: &mf_runtime::ExecutionDomains,
) -> TokenStream {
    let ident = format_ident!("FLOW_PLAN_{}", scope.to_uppercase());
    let indices: BTreeMap<_, _> = order
        .iter()
        .enumerate()
        .map(|(index, id)| (id, index))
        .collect();
    let dependencies = dependency_layout_tokens(definition, order);
    let connections = definition.edges.iter().map(|edge| {
        let from = indices[&edge.from_node];
        let to = indices[&edge.to_node];
        let output = LitStr::new(&edge.from_output, Span::call_site());
        let input = LitStr::new(&edge.to_input, Span::call_site());
        quote! { mf_runtime::FlowConnection {
            from_node: mf_runtime::NodeId::new(#from), to_node: mf_runtime::NodeId::new(#to),
            from_output: std::borrow::Cow::Borrowed(#output), to_input: std::borrow::Cow::Borrowed(#input),
        } }
    });
    let node_order = (0..order.len()).map(|index| quote! { mf_runtime::NodeId::new(#index) });
    let outputs = definition.outputs.iter().map(|output| {
        let index = indices[&output.node];
        let name = LitStr::new(&output.name, Span::call_site());
        let port = LitStr::new(&output.port, Span::call_site());
        let optional = output.optional;
        quote! { mf_runtime::FlowOutput {
            name: std::borrow::Cow::Borrowed(#name), node_id: mf_runtime::NodeId::new(#index),
            port: std::borrow::Cow::Borrowed(#port), optional: #optional,
        } }
    });
    let domains = domain_tokens(domains);
    quote! { pub static #ident: mf_runtime::FlowPlan = mf_runtime::FlowPlan {
        connections: std::borrow::Cow::Borrowed(&[#(#connections),*]), dependencies: std::borrow::Cow::Borrowed(&[#(#dependencies),*]),
        execution_order: std::borrow::Cow::Borrowed(&[#(#node_order),*]), outputs: std::borrow::Cow::Borrowed(&[#(#outputs),*]),
        execution_domains: #domains,
    }; }
}

#[cfg(feature = "codegen")]
fn collect_body_layouts(
    definition: &WorkflowDefinition,
    order: &[DefinitionId],
    scope: &str,
    layouts: &mut Vec<TokenStream>,
) -> Result<(), PlanError> {
    for (index, id) in order.iter().enumerate() {
        let node = definition
            .nodes
            .iter()
            .find(|node| &node.id == id)
            .expect("validated node ID");
        let body = if let Some(loop_definition) = &node.loop_definition {
            crate::loops::body_definition(&loop_definition.body, &definition.dependencies)
        } else if node.kind == ITERATION_KIND {
            let invalid = |message| {
                IterationSnafu {
                    definition_id: id.clone(),
                    message,
                }
                .build()
            };
            let config = parse_config(node)?;
            body_definition(definition, &config).map_err(invalid)?
        } else {
            continue;
        };
        let body_order = crate::compiler::structural_order_graph(&body)?;
        let indices: BTreeMap<_, _> = body_order
            .iter()
            .enumerate()
            .map(|(index, id)| (id.as_str(), index))
            .collect();
        let incoming = crate::compiler::incoming_dependencies(&body);
        let mut edges = Vec::new();
        for (target, id) in body_order.iter().enumerate() {
            for dependency in incoming.get(id.as_str()).into_iter().flatten() {
                edges.push((indices[dependency.source_node], target));
            }
        }
        let node_order: Vec<_> = (0..body_order.len()).map(mf_runtime::NodeId::new).collect();
        let domains =
            crate::partition_execution_domains(&node_order, &edges, &vec![false; body_order.len()]);
        let body_scope = format!("{scope}_{index}");
        layouts.push(task_layout_tokens(
            &body,
            &body_order,
            &body_scope,
            &domains,
        ));
        collect_body_layouts(&body, &body_order, &body_scope, layouts)?;
    }
    Ok(())
}
