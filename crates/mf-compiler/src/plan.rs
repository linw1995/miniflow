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
    pub typed_plan_json: String,
}

#[derive(Debug, Snafu)]
pub enum PlanError {
    #[snafu(display("invalid typed constructor for node `{definition_id}`: {source}"))]
    TypedConstructor {
        definition_id: DefinitionId,
        source: mf_runtime::TypedGenerationError,
    },
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

    /// Generate preparation and execution entry points for custom runners.
    #[cfg(feature = "codegen")]
    pub fn generate_artifacts(&self) -> Result<GeneratedWorkflowArtifacts, PlanError> {
        self.generate_artifacts_with_helpers(true)
    }

    /// Generate only the entry points used by the standalone runner.
    #[cfg(feature = "codegen")]
    pub fn generate_runner_artifacts(&self) -> Result<GeneratedWorkflowArtifacts, PlanError> {
        self.generate_artifacts_with_helpers(false)
    }

    #[cfg(feature = "codegen")]
    fn generate_artifacts_with_helpers(
        &self,
        include_helpers: bool,
    ) -> Result<GeneratedWorkflowArtifacts, PlanError> {
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
            ScopeBuild::Dynamic,
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
        let helpers = if include_helpers {
            quote! {
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

                pub fn run_workflow_in_context_with_options(
                    flow: &mf_runtime::Flow,
                    state: &mut mf_runtime::ExecutionContext,
                    options: mf_runtime::RuntimeOptions,
                ) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
                    mf_runtime::FlowRuntime::new(options).execute_in_context(flow, state)
                }
            }
        } else {
            TokenStream::new()
        };
        let preparation_body = if include_helpers {
            quote! { use snafu::ResultExt as _; let mut observation = observation; #(#preparations)* Ok(#root_flow) }
        } else {
            quote! { mf_prepare_generated_workflow!(registry, observation) }
        };
        let generated = quote! {
            include!(concat!(env!("OUT_DIR"), "/flow-plans.rs"));

            pub fn prepare_workflow(
                registry: &mf_runtime::NodeRegistry,
                observation: Option<&mut mf_runtime::RunObservation>,
            ) -> Result<mf_runtime::Flow, mf_compiler::WorkflowBuildError> {
                #preparation_body
            }

            #helpers

            pub fn run_workflow_in_context(
                flow: &mf_runtime::Flow,
                state: &mut mf_runtime::ExecutionContext,
            ) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
                mf_runtime::FlowRuntime::default().execute_in_context(flow, state)
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
    let (preparations, _, _) = generate_scope(
        definition,
        order,
        "stream",
        &[],
        None,
        None,
        ScopeBuild::PreparationOnly,
    )?;
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
enum ScopeBuild<'a> {
    PreparationOnly,
    Dynamic,
    Typed(&'a BTreeMap<DefinitionId, (TokenStream, LitStr)>),
}

#[cfg(feature = "codegen")]
fn generate_scope(
    definition: &WorkflowDefinition,
    order: &[DefinitionId],
    scope: &str,
    static_scope: &[String],
    enclosing: Option<&LoopDefinition>,
    preparation_error_override: Option<&TokenStream>,
    build: ScopeBuild<'_>,
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
        if let ScopeBuild::Typed(constructors) = &build
            && let Some((constructor, expected)) = constructors.get(id)
        {
            let handle = format_ident!("typed_root_{index}");
            preparations.push(quote! {
                let #handle = #constructor.inspect_err(|error| { #preparation_report })?;
                let mut #node_ident = mf_runtime::FlowNode::new(#id_lit, #handle.prepared());
                #inference_ident.resolve_node(&mut #node_ident, &[#(#bindings),*])
                    .inspect_err(|error| { #preparation_report })
                    .boxed().context(mf_compiler::WorkflowMetadataSnafu { definition_id: #id_lit })?;
                let expected = serde_json::from_str(#expected).context(mf_compiler::WorkflowInvalidEmbeddedConfigSnafu {
                    definition_id: mf_runtime::DefinitionId::from(#id_lit),
                })?;
                mf_runtime::verify_generated_metadata(&#node_ident.metadata, &expected)
                    .context(mf_compiler::WorkflowNodeConstructionSnafu { definition_id: mf_runtime::DefinitionId::from(#id_lit) })
                    .inspect_err(|error| { #preparation_report })?;
            });
            node_idents.push(node_ident);
            continue;
        }
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
    if !matches!(build, ScopeBuild::PreparationOnly) {
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
        ScopeBuild::Dynamic,
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
        self.generate_execution_plans_with_typed(registry, false)
    }

    pub fn generate_runner_execution_plans(
        &self,
        registry: &mf_runtime::NodeRegistry,
    ) -> Result<GeneratedExecutionArtifacts, PlanError> {
        self.generate_execution_plans_with_typed(registry, true)
    }

    fn generate_execution_plans_with_typed(
        &self,
        registry: &mf_runtime::NodeRegistry,
        standard: bool,
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
        let mut typed_plan = crate::TypedPlan::default();
        if let Some(execution) = &self.definition.execution {
            typed_plan.connections = self
                .definition
                .edges
                .iter()
                .map(|edge| crate::TypedConnectionReport {
                    source: edge.from_node.to_string(),
                    output: edge.from_output.clone(),
                    target: edge.to_node.to_string(),
                    input: edge.to_input.clone(),
                    fallback: Some(crate::TypedFallbackReason::UnsupportedMode),
                })
                .collect();
            typed_plan.connections.sort_by(|a, b| {
                (&a.source, &a.output, &a.target, &a.input)
                    .cmp(&(&b.source, &b.output, &b.target, &b.input))
            });
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
            typed_plan = crate::plan_typed_segments(&flow, standard, true);
            if standard {
                layouts.push(typed_preparation_tokens(self, &flow, &typed_plan)?);
            }
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
            typed_plan_json: serde_json::to_string_pretty(&typed_plan).context(SerializeSnafu)?,
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
fn rust_value_tokens(value: &mf_runtime::RustValueType) -> TokenStream {
    use mf_runtime::RustValueType::*;
    match value {
        Boolean => quote!(bool),
        Int64 => quote!(i64),
        Uint64 => quote!(u64),
        Usize => quote!(usize),
        Float32 => quote!(f32),
        Float64 => quote!(f64),
        String => quote!(std::string::String),
        Shared => quote!(mf_runtime::ValueRef),
        List(inner) => {
            let inner = rust_value_tokens(inner);
            quote!(std::vec::Vec<#inner>)
        }
        Map(inner) => {
            let inner = rust_value_tokens(inner);
            quote!(std::collections::BTreeMap<std::string::String, #inner>)
        }
        Defaulted(inner) => rust_value_tokens(inner),
        Nullable(inner) => {
            let inner = rust_value_tokens(inner);
            quote!(mf_runtime::Nullable<#inner>)
        }
        SharedPayload(inner) => {
            let inner = rust_value_tokens(inner);
            quote!(mf_runtime::Shared<#inner>)
        }
        Optional(inner) => {
            let inner = rust_value_tokens(inner);
            quote!(std::option::Option<#inner>)
        }
    }
}

#[cfg(feature = "codegen")]
fn typed_preparation_tokens(
    workflow: &CompiledWorkflow,
    flow: &mf_runtime::Flow,
    plan: &crate::TypedPlan,
) -> Result<TokenStream, PlanError> {
    let typed_positions: BTreeSet<_> = plan
        .segments
        .iter()
        .flat_map(|segment| segment.positions.iter().copied())
        .collect();
    let mut constructors = BTreeMap::new();
    for &position in &typed_positions {
        let node = &flow.nodes()[flow.execution_order()[position].index()];
        let generation = node
            .metadata
            .typed_generation
            .as_ref()
            .expect("planned typed provider");
        let alias = generation
            .constructor
            .dependency_alias(&workflow.definition.dependencies)
            .with_context(|_| TypedConstructorSnafu {
                definition_id: node.definition_id.clone(),
            })?;
        // Dependency projects deliberately use ordinal Rust crate names, independently of user aliases.
        let index = workflow
            .definition
            .dependencies
            .keys()
            .position(|key| key == alias)
            .expect("selected alias");
        let provider = format_ident!("node_{index}");
        let parts: Vec<_> = generation
            .constructor
            .path
            .iter()
            .map(|part| format_ident!("{part}"))
            .collect();
        let id = LitStr::new(node.definition_id.as_str(), Span::call_site());
        let definition = workflow
            .definition
            .nodes
            .iter()
            .find(|definition| definition.id == node.definition_id)
            .expect("validated definition");
        let kind = LitStr::new(&definition.kind, Span::call_site());
        let config = LitStr::new(
            &serde_json::to_string(&definition.config).context(SerializeSnafu)?,
            Span::call_site(),
        );
        let input_types: Vec<_> = generation
            .inputs
            .iter()
            .map(|field| rust_value_tokens(&field.rust_type))
            .collect();
        let output_types: Vec<_> = generation
            .outputs
            .iter()
            .map(|field| rust_value_tokens(&field.rust_type))
            .collect();
        let input_names: Vec<_> = generation
            .inputs
            .iter()
            .map(|field| LitStr::new(&field.port.name, Span::call_site()))
            .collect();
        let output_names: Vec<_> = generation
            .outputs
            .iter()
            .map(|field| LitStr::new(&field.port.name, Span::call_site()))
            .collect();
        let expected = LitStr::new(
            &serde_json::to_string(&node.metadata).context(SerializeSnafu)?,
            Span::call_site(),
        );
        constructors.insert(node.definition_id.clone(), (quote! {
            (|| -> Result<_, mf_compiler::WorkflowBuildError> {
                use snafu::OptionExt as _;
                registry.get(#kind).context(mf_compiler::WorkflowUnknownKindSnafu {
                    definition_id: mf_runtime::DefinitionId::from(#id), kind: #kind,
                })?;
                let config = serde_json::from_str(#config).context(mf_compiler::WorkflowInvalidEmbeddedConfigSnafu {
                    definition_id: mf_runtime::DefinitionId::from(#id),
                })?;
                let handle = ::#provider::#(#parts)::* (config).context(mf_compiler::WorkflowNodeConstructionSnafu {
                    definition_id: mf_runtime::DefinitionId::from(#id),
                })?;
                fn verify_fields<N: mf_runtime::TypedTaskNode>(_: &mf_runtime::TypedTaskHandle<N>)
                where N::Input: mf_runtime::TypedNodeValue<Fields = (#(#input_types,)*)>,
                    N::Output: mf_runtime::TypedNodeValue<Fields = (#(#output_types,)*)> {
                    const {
                        assert!(mf_runtime::port_names_match(<N::Input as mf_runtime::TypedNodeValue>::PORT_NAMES, &[#(#input_names),*]), "typed constructor input ports differ");
                        assert!(mf_runtime::port_names_match(<N::Output as mf_runtime::TypedNodeValue>::PORT_NAMES, &[#(#output_names),*]), "typed constructor output ports differ");
                    }
                }
                verify_fields(&handle);
                Ok(handle)
            })()
        }, expected));
    }
    let (preparations, _, root) = generate_scope(
        &workflow.definition,
        &workflow.execution_order,
        "root",
        &[],
        None,
        None,
        ScopeBuild::Typed(&constructors),
    )?;
    let mut bindings = Vec::new();
    for domain in flow.execution_domains().domains() {
        let segments: Vec<_> = plan
            .segments
            .iter()
            .filter(|segment| segment.domain == domain.id)
            .collect();
        if segments.is_empty() {
            continue;
        }
        let mut statements = Vec::new();
        for &position in domain.positions.iter() {
            statements.push(if position == 0 {
                quote! { if state.scope_exit_requested() { return Ok(()); } }
            } else { quote! { if #position > state.scope_exit_cutoff() || state.scope_exit_requested() { return Ok(()); } } });
            let Some(segment) = segments
                .iter()
                .find(|segment| segment.positions.contains(&position))
            else {
                statements.push(quote! { flow.execute_position(#position, state)?; });
                continue;
            };
            let node = &flow.nodes()[flow.execution_order()[position].index()];
            let generation = node.metadata.typed_generation.as_ref().expect("typed node");
            let handle = format_ident!("typed_root_{position}");
            let offset = segment
                .positions
                .iter()
                .position(|index| *index == position)
                .expect("segment membership");
            let mut transfers = Vec::new();
            let input = if offset == 0 {
                quote! { #handle.decode_inputs(_inputs)? }
            } else {
                let previous = segment.positions[offset - 1];
                let previous_node = &flow.nodes()[flow.execution_order()[previous].index()];
                let previous_generation = previous_node
                    .metadata
                    .typed_generation
                    .as_ref()
                    .expect("typed predecessor");
                let fields: Vec<_> = previous_generation
                    .outputs
                    .iter()
                    .enumerate()
                    .map(|(index, _)| format_ident!("_mf_field_{previous}_{index}"))
                    .collect();
                let arguments = generation
                    .inputs
                    .iter()
                    .map(|field| {
                        if let Some(edge) = flow.connections().iter().find(|edge| {
                            edge.to_node == flow.execution_order()[position]
                                && edge.to_input == field.port.name
                        }) {
                            transfers.push(LitStr::new(&field.port.name, Span::call_site()));
                            let index = previous_generation
                                .outputs
                                .iter()
                                .position(|field| field.port.name == edge.from_output)
                                .expect("planned output mapping");
                            let field = &fields[index];
                            quote! { #field }
                        } else {
                            quote! { None }
                        }
                    })
                    .collect::<Vec<_>>();
                let slot = format_ident!("_mf_fields_{previous}");
                quote! { {
                    let (#(#fields,)*) = #slot.take().expect("validated typed predecessor presence");
                    #handle.input_from_fields((#(#arguments,)*))
                } }
            };
            let publication = if offset + 1 == segment.positions.len() {
                quote! { mf_runtime::GeneratedNodeResult::encoded(result) }
            } else {
                let slot = format_ident!("_mf_fields_{position}");
                statements.push(quote! { let mut #slot = None; });
                quote! {
                    let publication = mf_runtime::GeneratedNodeResult::typed(&result)?;
                    #slot = Some(result.outputs.into_fields());
                    Ok(publication)
                }
            };
            statements.push(quote! {
                flow.execute_generated_position(#position, &[#(#transfers),*], state, |_inputs, ctx| {
                    let input = #input;
                    let result = #handle.execute(input, ctx)?;
                    #publication
                })?;
            });
        }
        let id = domain.id;
        bindings.push(quote! { (#id, std::sync::Arc::new(move |flow: &mf_runtime::Flow, state: &mut mf_runtime::ExecutionContext| {
            #(#statements)*
            Ok(())
        }) as std::sync::Arc<mf_runtime::GeneratedDomainExecutor>) });
    }
    let registry_binding = if workflow.definition.nodes.is_empty() {
        quote!(let _registry = $registry;)
    } else {
        quote!(let registry = $registry;)
    };
    let typed_import = if typed_positions.is_empty() {
        quote!()
    } else {
        quote! { use mf_runtime::TypedNodeValue as _; }
    };
    // Expansion is requested only by standalone entry points; custom runners do not instantiate unused typed bodies.
    Ok(quote! {
        #[macro_export]
        macro_rules! mf_prepare_generated_workflow {
            ($registry:expr, $observation:expr) => {{
                use snafu::ResultExt as _;
                #typed_import
                #registry_binding
                let mut observation = $observation;
                #(#preparations)*
                let #root = #root.with_generated_domains(vec![#(#bindings),*]);
                Ok(#root)
            }};
        }
    })
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
