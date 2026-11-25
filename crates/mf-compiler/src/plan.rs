use crate::DefinitionId;
#[cfg(feature = "codegen")]
use crate::WorkflowDefinition;
#[cfg(feature = "codegen")]
use proc_macro2::{Span, TokenStream};
#[cfg(feature = "codegen")]
use quote::{format_ident, quote};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::collections::BTreeMap;
#[cfg(feature = "codegen")]
use std::collections::BTreeSet;
#[cfg(feature = "codegen")]
use syn::LitStr;

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledWorkflow {
    pub graph: mf_runtime::CompiledGraph,
}

#[cfg(feature = "codegen")]
#[derive(Clone, Copy, Debug, Default)]
pub struct CodegenOptions {
    /// Keep intermediate values available to host context readers.
    pub preserve_intermediate_outputs: bool,
}

#[cfg(feature = "codegen")]
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedWorkflowArtifacts {
    pub rust_source: String,
    pub manifest: mf_runtime::WorkflowManifest,
    pub typed_plan: crate::TypedPlan,
}

#[derive(Debug, Snafu)]
pub enum PlanError {
    #[snafu(transparent)]
    Definition {
        source: mf_runtime::DefinitionParseError,
    },
    #[snafu(display("node target `{definition_id}` references unknown context output `{output}`"))]
    UnknownTargetReference {
        definition_id: DefinitionId,
        output: String,
    },
    #[snafu(transparent)]
    TargetInputs {
        source: mf_runtime::WorkflowInputError,
    },
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
    Sources { source: crate::FlowResolutionError },
    #[snafu(transparent)]
    Compilation {
        #[snafu(source(from(crate::WorkflowCompileError, Box::new)))]
        source: Box<crate::WorkflowCompileError>,
    },
    #[snafu(transparent)]
    Construction { source: crate::WorkflowBuildError },
    #[snafu(
        display("could not serialize compiled workflow: {source}"),
        visibility(pub)
    )]
    Serialize { source: serde_json::Error },
    #[snafu(display("compiled plan references unknown node `{definition_id}`"))]
    UnknownNode { definition_id: DefinitionId },
    #[snafu(display("compiled plan has an incomplete or duplicate execution order"))]
    InvalidExecutionOrder,
    #[cfg(feature = "codegen")]
    #[snafu(display("generated Rust code is invalid: {source}"), visibility(pub))]
    GeneratedSyntax { source: syn::Error },
}

#[cfg(feature = "codegen")]
pub fn generate_embedded_source(
    workflow: &crate::FlowSourceBundle,
) -> Result<String, crate::PlanError> {
    let entry = crate::flow_sources::portable_embedded_path(&workflow.entry)?;
    let sources = workflow
        .sources
        .iter()
        .map(|(path, source)| {
            let path = crate::flow_sources::portable_embedded_path(path)?;
            Ok(quote!((std::path::PathBuf::from(#path), #source.to_owned())))
        })
        .collect::<Result<Vec<_>, PlanError>>()?;
    let generated = quote::quote! {
        pub fn plan(
            registry: &mf_runtime::NodeRegistry,
        ) -> Result<mf_compiler::CompiledWorkflow, mf_compiler::PlanError> {
            let workflow = mf_compiler::FlowSourceBundle {
                entry: std::path::PathBuf::from(#entry),
                sources: std::collections::BTreeMap::from([#(#sources),*]),
            };
            let resolved = workflow.resolve(registry)?;
            Ok(mf_compiler::plan_resolved_workflow(&resolved)?)
        }

        pub fn manifest(
            registry: &mf_runtime::NodeRegistry,
        ) -> Result<mf_runtime::WorkflowManifest, mf_compiler::PlanError> {
            plan(registry)?.manifest(registry)
        }

        pub fn prepare_workflow(
            registry: &mf_runtime::NodeRegistry,
            observation: Option<&mut mf_runtime::RunObservation>,
        ) -> Result<mf_runtime::Flow, mf_compiler::PlanError> {
            Ok(mf_compiler::instantiate_compiled_with_observation(
                &plan(registry)?, registry, observation,
            )?)
        }

        pub fn prepare_stream(
            registry: &mf_runtime::NodeRegistry,
        ) -> Result<mf_runtime::PreparedStream, mf_compiler::PlanError> {
            Ok(mf_compiler::instantiate_stream(&plan(registry)?, registry)?)
        }
    };
    let syntax: syn::File = syn::parse2(generated).context(GeneratedSyntaxSnafu)?;
    Ok(prettyplease::unparse(&syntax))
}

impl CompiledWorkflow {
    pub fn manifest(
        &self,
        registry: &mf_runtime::NodeRegistry,
    ) -> Result<mf_runtime::WorkflowManifest, PlanError> {
        let (resolved, builder) = crate::compiler::prepare_compiled_plan(self, registry)?;
        let targets = resolved.node_targets(builder.nodes())?;
        let schema = if let Some(execution) = &resolved.graph.definition.execution {
            builder
                .into_stream(execution.clone())
                .map_err(crate::WorkflowBuildError::from)?
                .plan()
                .input_schema()
                .clone()
        } else {
            builder
                .into_tasks()
                .map_err(crate::WorkflowBuildError::from)?
                .input_schema()
                .clone()
        };
        let mut manifest = resolved.manifest_with_schema(schema)?;
        manifest.descriptor.node_targets = targets;
        manifest.validate()?;
        Ok(manifest)
    }

    fn manifest_with_schema(
        &self,
        input_schema: mf_runtime::WorkflowInputSchema,
    ) -> Result<mf_runtime::WorkflowManifest, PlanError> {
        Ok(mf_runtime::WorkflowManifest::from_graph(
            &self.graph,
            input_schema,
        )?)
    }

    /// Describe isolated targets using metadata already prepared in execution order.
    pub fn node_targets(
        &self,
        nodes: &[mf_runtime::FlowNode],
    ) -> Result<Vec<mf_runtime::NodeTarget>, PlanError> {
        crate::validate_execution(&self.graph.definition)
            .map_err(crate::WorkflowBuildError::from)?;
        ensure!(
            nodes.len() == self.graph.execution_order.len(),
            InvalidExecutionOrderSnafu
        );
        let original = crate::describe_compiled(self)?;
        let mut bodies = BTreeMap::<String, Vec<_>>::new();
        for body in &original.loop_bodies {
            if let Some(root) = body.path.first() {
                let owner = if original.nodes.iter().any(|node| node.id == *root) {
                    root.clone()
                } else {
                    mf_telemetry::event::ScopePathEntry::binding_owner(root)
                        .unwrap_or_else(|| root.clone())
                };
                bodies.entry(owner).or_default().push(body.clone());
            }
        }
        let output_types: BTreeMap<_, _> = nodes
            .iter()
            .flat_map(|node| {
                node.metadata.output_ports().map(move |port| {
                    (
                        mf_runtime::output_id(node.definition_id.as_str(), &port.name),
                        port.value_type.clone(),
                    )
                })
            })
            .collect();
        nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                self.graph
                    .relations
                    .instruction(&node.definition_id)
                    .is_none()
            })
            .map(|(index, node)| {
                let id = node.definition_id.to_string();
                ensure!(
                    self.graph.execution_order.get(index) == Some(&node.definition_id),
                    InvalidExecutionOrderSnafu
                );
                let kind = match node.node.as_ref() {
                    Some(
                        mf_runtime::NodeExecution::Task(_)
                        | mf_runtime::NodeExecution::TaskStream(_),
                    ) => mf_runtime::NodeTargetKind::Task,
                    Some(mf_runtime::NodeExecution::Event(_)) => mf_runtime::NodeTargetKind::Event,
                    Some(mf_runtime::NodeExecution::Stream(_)) => {
                        mf_runtime::NodeTargetKind::Stream
                    }
                    None => {
                        return UnknownNodeSnafu {
                            definition_id: node.definition_id.clone(),
                        }
                        .fail();
                    }
                };
                let inputs =
                    crate::workflow_input_schema(std::iter::once((node, true)), |_, _| false)?;
                let mut description = mf_telemetry::description::WorkflowDescription {
                    order_edges: Vec::new(),
                    version: mf_telemetry::description::WorkflowDescriptionVersion::CURRENT,
                    workflow_id: original.workflow_id.clone(),
                    execution: None,
                    nodes: vec![original.nodes[index].clone()],
                    data_edges: Vec::new(),
                    control_edges: Vec::new(),
                    execution_order: vec![id.clone()],
                    loop_bodies: bodies.get(id.as_str()).cloned().unwrap_or_default(),
                    clusters: Vec::new(),
                };
                if original.version.supports_flow_invocations() {
                    description.version = original.version;
                    description.execution = Some(mf_telemetry::description::ExecutionDescription {
                        mode: mf_telemetry::description::ExecutionMode::Single,
                    });
                } else if original.version.supports_clusters() {
                    description.version =
                        mf_telemetry::description::WorkflowDescriptionVersion::V2026_10_23;
                    description.execution = Some(mf_telemetry::description::ExecutionDescription {
                        mode: mf_telemetry::description::ExecutionMode::Single,
                    });
                } else if self
                    .graph
                    .children
                    .get(&node.definition_id)
                    .into_iter()
                    .flat_map(BTreeMap::values)
                    .any(requires_scope_order_description)
                {
                    description.version =
                        mf_telemetry::description::WorkflowDescriptionVersion::V2026_10_19;
                    description.execution = Some(mf_telemetry::description::ExecutionDescription {
                        mode: mf_telemetry::description::ExecutionMode::Single,
                    });
                }
                description.workflow_id = original
                    .workflow_id
                    .for_node_target(&id)
                    .context(mf_runtime::GraphObservationSnafu)
                    .context(crate::compiler::GraphSnafu)?;
                description
                    .validate()
                    .context(mf_runtime::GraphObservationSnafu)
                    .context(crate::compiler::GraphSnafu)?;
                let context = node
                    .metadata
                    .context_references
                    .iter()
                    .map(|reference| {
                        let value_type = output_types.get(&reference.output).context(
                            UnknownTargetReferenceSnafu {
                                definition_id: node.definition_id.clone(),
                                output: &reference.output,
                            },
                        )?;
                        Ok((reference.output.clone(), value_type.clone()))
                    })
                    .collect::<Result<_, PlanError>>()?;
                Ok(mf_runtime::NodeTarget {
                    id,
                    kind,
                    inputs,
                    outputs: mf_runtime::target_ports(&node.metadata.ports.outputs),
                    worker_limit: self
                        .graph
                        .definition
                        .execution
                        .as_ref()
                        .and_then(|execution| {
                            std::num::NonZeroUsize::new(execution.limits.workers)
                        }),
                    context,
                    original_workflow_id: original.workflow_id.clone(),
                    description,
                })
            })
            .collect()
    }

    pub fn start_stream_observation(
        &self,
        observer: &mf_telemetry::observation::Observer,
        run_id: mf_telemetry::identity::RunId,
    ) -> Result<mf_runtime::StreamObservation, crate::DescriptionError> {
        let mut description = crate::describe_compiled(self)?;
        if let Some(execution) = &mut description.execution {
            execution.mode = mf_telemetry::description::ExecutionMode::Stream;
        }
        observer
            .start_stream(description, run_id)
            .context(mf_runtime::GraphObservationSnafu)
            .context(crate::compiler::GraphSnafu)
    }

    pub fn start_observation(
        &self,
        observer: &mf_telemetry::observation::Observer,
        run_id: mf_telemetry::identity::RunId,
    ) -> Result<mf_runtime::RunObservation, crate::DescriptionError> {
        ensure!(
            self.graph.definition.execution.is_none(),
            crate::compiler::InvalidObservationSnafu {
                message: "use start_stream_observation for streaming workflows",
            }
        );
        observer
            .start_with_description(crate::describe_compiled(self)?, run_id)
            .context(mf_runtime::GraphObservationSnafu)
            .context(crate::compiler::GraphSnafu)
    }

    /// Emit the build-only graph handoff as native Rust, never as a serialized compiler IR.
    #[cfg(feature = "codegen")]
    pub fn generate_plan_source(&self) -> Result<String, PlanError> {
        crate::describe_compiled(self)?;
        let graph = crate::literals::graph(&self.graph)?;
        let source = quote! {
            pub fn compiled_workflow() -> Result<mf_compiler::CompiledWorkflow, mf_runtime::DefinitionParseError> {
                Ok(mf_compiler::CompiledWorkflow { graph: #graph })
            }
        };
        let syntax = syn::parse2(source).context(GeneratedSyntaxSnafu)?;
        Ok(prettyplease::unparse(&syntax))
    }
}

fn requires_scope_order_description(graph: &mf_runtime::CompiledGraph) -> bool {
    !graph.definition.order_edges.is_empty()
        || graph
            .relations
            .scope
            .as_ref()
            .is_some_and(|frame| !frame.is_stateful())
        || graph
            .children
            .values()
            .flat_map(BTreeMap::values)
            .any(requires_scope_order_description)
}

#[cfg(feature = "codegen")]
pub fn child_scope_name(
    scope: &str,
    index: usize,
    binding_index: usize,
    node: &crate::NodeDefinition,
    name: &str,
    body: &mf_runtime::CompiledGraph,
) -> String {
    if body.child_scope_key(node, name) == node.id.as_str() {
        format!("{scope}_{index}")
    } else {
        format!("{scope}_{index}_binding_{binding_index}")
    }
}

#[cfg(feature = "codegen")]
impl CompiledWorkflow {
    /// Emits immutable layouts while linked providers are available to the generated Cargo build.
    pub fn generate_artifacts(
        &self,
        registry: &mf_runtime::NodeRegistry,
        options: CodegenOptions,
    ) -> Result<GeneratedWorkflowArtifacts, PlanError> {
        ensure!(
            self.graph.execution_order
                == crate::compiler::structural_order_graph(&self.graph.definition)?,
            InvalidExecutionOrderSnafu
        );
        crate::compiler::validate_compiled_scope_orders(self, registry)?;
        let mut body_layouts = BTreeMap::new();
        let (nodes, order) = crate::compiler::prepare_compiled_graph_with_layouts(
            &self.graph,
            registry,
            Some(&mut body_layouts),
        )?;
        if order != self.graph.execution_order {
            return InvalidExecutionOrderSnafu.fail();
        }
        let mut resolved = self.clone();
        resolved.graph.definition =
            crate::compiler::resolve_execution(&self.graph.definition, &nodes)?;
        resolved.graph.execution_order = order.clone();
        resolved.generate_resolved_artifacts(nodes, order, body_layouts, options)
    }

    fn generate_resolved_artifacts(
        &self,
        nodes: Vec<crate::FlowNode>,
        order: Vec<DefinitionId>,
        mut body_layouts: crate::compiler::ScopeLayouts,
        options: CodegenOptions,
    ) -> Result<GeneratedWorkflowArtifacts, PlanError> {
        let node_targets = self.node_targets(&nodes)?;
        let flow = crate::FlowBuilder::prepare(
            nodes,
            self.graph.definition.edges.clone(),
            order.clone(),
            self.graph.definition.outputs.clone(),
        )
        .and_then(|flow| flow.with_control_edges(self.graph.definition.control_edges.clone()))
        .and_then(|flow| flow.with_order_edges(self.graph.definition.order_edges.clone()))
        .map_err(crate::WorkflowBuildError::from)?
        .with_root_scope(self.graph.relations.root.clone());
        let flow = if self.graph.definition.version.supports_startup_inputs() {
            flow.with_workflow_inputs()
                .map_err(crate::WorkflowBuildError::from)?
        } else {
            flow
        };
        let schema = flow.input_schema().clone();
        let root_name = if self.graph.definition.execution.is_some() {
            "stream"
        } else {
            "root"
        };
        let root_layout = body_layouts
            .get_mut(&Vec::new())
            .expect("prepared root contracts");
        root_layout.input_schema = schema.clone();
        let root_schema = format_ident!("input_schema_{root_name}");
        let indices = self
            .graph
            .execution_order
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let id = node.as_str();
                quote!(#id => Ok(#index),)
            });
        let mut layouts = vec![
            crate::fixed::bindings(root_name, root_layout),
            quote! {
                pub fn compiled_input_schema() -> mf_runtime::WorkflowInputSchema { #root_schema() }
                pub fn compiled_node_index(id: &str) -> Result<usize, mf_runtime::NodeTargetError> {
                    match id { #(#indices)* _ => mf_runtime::NodeTargetUnknownSnafu { id }.fail() }
                }
            },
        ];
        if !self.graph.children.is_empty() {
            let workflow_id = self
                .graph
                .workflow_id()
                .context(crate::compiler::GraphSnafu)?;
            let identity = LitStr::new(workflow_id.as_str(), Span::call_site());
            // This graph already has its published execution mode. Do not prepare
            // providers (or consult the authored, unresolved graph) to recover its ID.
            layouts.push(quote! {
                fn mf_compiled_flow_origin(
                    path: Vec<String>,
                ) -> Result<mf_runtime::FlowOrigin, mf_runtime::generated::BindError> {
                    use snafu::ResultExt as _;
                    let workflow_id = mf_telemetry::identity::WorkflowId::try_from(#identity.to_owned())
                        .context(mf_runtime::generated::IdentitySnafu)?;
                    Ok(mf_runtime::FlowOrigin { workflow_id, path })
                }
            });
        }
        let scheduled = body_layouts
            .iter()
            .filter(|(_, layout)| layout.stream_plan.is_some())
            .map(|(path, _)| path.iter().map(ToString::to_string).collect())
            .collect();
        let mut typed_plan = crate::TypedPlan::default();
        if let Some(execution) = &self.graph.definition.execution {
            typed_plan.connections = self
                .graph
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
            layouts.push(crate::fixed::stream_preparation(self, &scheduled)?);
            layouts.push(stream_program_tokens(
                prepared.plan(),
                &self.graph.definition,
                &order,
            ));
            layouts.push(stream_guard_tokens(
                &self.graph.definition,
                &order,
                prepared.plan(),
            ));
            collect_body_layouts(
                &self.graph,
                "stream",
                &mut Vec::new(),
                &body_layouts,
                &mut layouts,
            )?;
        } else {
            let flow = flow.into_tasks().map_err(crate::WorkflowBuildError::from)?;
            typed_plan =
                crate::plan_typed_segments(&flow, !options.preserve_intermediate_outputs, true);
            layouts.push(typed_preparation_tokens(
                self,
                &flow,
                &typed_plan,
                &scheduled,
            )?);
            layouts.push(task_layout_tokens(
                &self.graph.definition,
                &order,
                "root",
                flow.execution_domains(),
            ));
            collect_body_layouts(
                &self.graph,
                "root",
                &mut Vec::new(),
                &body_layouts,
                &mut layouts,
            )?;
        }
        let syntax: syn::File =
            syn::parse2(quote! { #(#layouts)* }).context(GeneratedSyntaxSnafu)?;
        let mut manifest = self.manifest_with_schema(schema)?;
        manifest.descriptor.node_targets = node_targets;
        manifest.validate()?;
        Ok(GeneratedWorkflowArtifacts {
            rust_source: prettyplease::unparse(&syntax),
            manifest,
            typed_plan,
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
            let values = dependencies.iter().filter_map(|dependency| {
                let source = LitStr::new(dependency.source_node(), Span::call_site());
                let output = dependency.source_output()?;
                let key = LitStr::new(
                    &mf_runtime::output_id(dependency.source_node(), output),
                    Span::call_site(),
                );
                let output = LitStr::new(output, Span::call_site());
                let input = match dependency.input() {
                    Some(input) => {
                        let input = LitStr::new(input, Span::call_site());
                        quote! { Some(std::borrow::Cow::Borrowed(#input)) }
                    }
                    None => quote! { None },
                };
                let exclusive = dependency.exclusive();
                Some(quote! { mf_runtime::FlowDependency { exclusive: #exclusive,
                    input: #input,
                    source_node: std::borrow::Cow::Borrowed(#source),
                    source_output: std::borrow::Cow::Borrowed(#output),
                    source_key: std::borrow::Cow::Borrowed(#key),
                } })
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
    scheduled: &crate::fixed::ScheduledScopes,
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
            .dependency_alias(&workflow.graph.definition.dependencies)
            .with_context(|_| TypedConstructorSnafu {
                definition_id: node.definition_id.clone(),
            })?;
        // Dependency projects deliberately use ordinal Rust crate names, independently of user aliases.
        let index = workflow
            .graph
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
            .graph
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
        constructors.insert(node.definition_id.clone(), quote! {
            (|| -> Result<_, mf_runtime::generated::BindError> {
                let (_, config) = mf_runtime::generated::configuration(registry, #id, #kind, #config, &[])?;
                let handle = ::#provider::#(#parts)::* (config).context(mf_runtime::generated::NodeSnafu {
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
        });
    }
    let (preparations, _, root) = crate::fixed::scope(
        &workflow.graph,
        "root",
        &[],
        &constructors,
        true,
        scheduled,
        None,
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
                statements.push(task_call_tokens(position, &[], None));
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
            let invoke = quote! { |_inputs, ctx| {
                let input = #input;
                let result = #handle.execute(input, ctx)?;
                #publication
            } };
            statements.push(task_call_tokens(position, &transfers, Some(invoke)));
        }
        let id = domain.id;
        bindings.push(quote! { (#id, std::sync::Arc::new(move |flow: &mf_runtime::Flow, state: &mut mf_runtime::ExecutionContext| {
            #(#statements)*
            Ok(())
        }) as std::sync::Arc<mf_runtime::GeneratedDomainExecutor>) });
    }
    let typed_import = if typed_positions.is_empty() {
        quote!()
    } else {
        quote! { use mf_runtime::TypedNodeValue as _; }
    };
    // Expansion is requested only by standalone entry points; custom runners do not instantiate unused typed bodies.
    Ok(quote! {
        pub fn prepare_workflow(
            registry: &mf_runtime::NodeRegistry,
            mut observation: Option<&mut mf_runtime::RunObservation>,
        ) -> Result<mf_runtime::Flow, mf_runtime::generated::BindError> {
            #[allow(unused_imports)]
            use snafu::ResultExt as _;
            #typed_import
            let _ = registry;
            let _ = &mut observation;
            #(#preparations)*
            Ok(#root.with_generated_domains(vec![#(#bindings),*]))
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
        let visibility = domain.snapshot_visibility.keys();
        quote! { mf_runtime::ExecutionDomain {
            id: #id, first_position: #first,
            snapshot_visibility: mf_runtime::OutputVisibility::Borrowed(&[#(#visibility),*]),
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
        let exclusive = edge.exclusive;
        let from = indices[&edge.from_node];
        let to = indices[&edge.to_node];
        let output = LitStr::new(&edge.from_output, Span::call_site());
        let input = LitStr::new(&edge.to_input, Span::call_site());
        quote! { mf_runtime::FlowConnection { exclusive: #exclusive,
            from_node: mf_runtime::NodeId::new(#from), to_node: mf_runtime::NodeId::new(#to),
            from_output: std::borrow::Cow::Borrowed(#output), to_input: std::borrow::Cow::Borrowed(#input),
        } }
    });
    let node_order = (0..order.len()).map(|index| quote! { mf_runtime::NodeId::new(#index) });
    let outputs = definition.outputs.iter().map(|output| {
        let index = indices[&output.node];
        let name = LitStr::new(&output.name, Span::call_site());
        let port = LitStr::new(&output.port, Span::call_site());
        let key = LitStr::new(
            &mf_runtime::output_id(output.node.as_str(), &output.port),
            Span::call_site(),
        );
        let optional = output.optional;
        quote! { mf_runtime::FlowOutput {
            name: std::borrow::Cow::Borrowed(#name), node_id: mf_runtime::NodeId::new(#index),
            port: std::borrow::Cow::Borrowed(#port), optional: #optional,
            key: std::borrow::Cow::Borrowed(#key),
        } }
    });
    let guards = task_guard_tokens(definition, order, scope);
    let domains = domain_tokens(domains);
    quote! { #guards pub static #ident: mf_runtime::FlowPlan = mf_runtime::FlowPlan {
        connections: std::borrow::Cow::Borrowed(&[#(#connections),*]), dependencies: std::borrow::Cow::Borrowed(&[#(#dependencies),*]),
        execution_order: std::borrow::Cow::Borrowed(&[#(#node_order),*]), outputs: std::borrow::Cow::Borrowed(&[#(#outputs),*]),
        execution_domains: #domains,
    }; }
}

#[cfg(feature = "codegen")]
fn subgraph_stream_tokens(
    scope: &str,
    id: &DefinitionId,
    outputs: &[mf_runtime::SubgraphStreamOutput],
) -> Result<TokenStream, PlanError> {
    let bind = format_ident!("bind_stream_outputs_{scope}");
    let id = LitStr::new(id.as_str(), Span::call_site());
    let bindings = outputs
        .iter()
        .map(|output| {
            let node = output.node.index();
            let optional = output.optional;
            let name = LitStr::new(&output.name, Span::call_site());
            let port = crate::literals::port(&output.port);
            Ok(quote! { mf_runtime::SubgraphStreamOutput {
                node:mf_runtime::NodeId::new(#node), name:#name.into(),
                optional: #optional,
                port: #port,
            } })
        })
        .collect::<Result<Vec<_>, PlanError>>()?;
    Ok(quote! {
        fn #bind(flow:mf_runtime::Flow) -> Result<mf_runtime::Flow,mf_runtime::generated::BindError> {
            use snafu::ResultExt as _;
            flow.with_stream_outputs(vec![#(#bindings),*]).context(mf_runtime::generated::NodeSnafu {definition_id:#id})
        }
    })
}

#[cfg(feature = "codegen")]
fn collect_body_layouts(
    graph: &mf_runtime::CompiledGraph,
    scope: &str,
    path: &mut Vec<DefinitionId>,
    body_layouts: &crate::compiler::ScopeLayouts,
    layouts: &mut Vec<TokenStream>,
) -> Result<(), PlanError> {
    for (index, id) in graph.execution_order.iter().enumerate() {
        let node = graph
            .definition
            .nodes
            .iter()
            .find(|node| &node.id == id)
            .expect("validated node ID");
        for (binding_index, (name, body)) in
            graph.children.get(id).into_iter().flatten().enumerate()
        {
            let mut body = body.clone();
            path.push(body.child_scope_key(node, name).into());
            let layout = &body_layouts[path.as_slice()];
            let streams = &layout.streams;
            layouts.push(crate::fixed::bindings(
                &child_scope_name(scope, index, binding_index, node, name, &body),
                layout,
            ));
            let body_scope = child_scope_name(scope, index, binding_index, node, name, &body);
            let bind = format_ident!("bind_child_{body_scope}");
            let schema = format_ident!("input_schema_{body_scope}");
            let interface = format_ident!("interface_{body_scope}");
            let owner = node.id.as_str();
            if let Some(plan) = &layout.stream_plan {
                let module = format_ident!("scheduled_{body_scope}");
                let program = stream_program_tokens(plan, &body.definition, &body.execution_order);
                let guards = stream_guard_tokens(&body.definition, &body.execution_order, plan);
                let limits = &plan.execution().limits;
                let workers = limits.workers;
                let pending = limits.max_pending_messages;
                layouts.push(quote! { mod #module { use super::*; #program #guards
                    pub(super) fn prepare(nodes: Vec<mf_runtime::FlowNode>) -> mf_runtime::PreparedStream {
                        let execution = mf_runtime::StreamExecution { limits: mf_runtime::StreamLimits { workers: #workers, max_pending_messages: #pending } };
                        bind_stream_guards(mf_runtime::PreparedStream::from_plan(execution, nodes, stream_program(), #schema()))
                    }
                } });
                layouts.push(quote! {
                    fn #bind(nodes: Vec<mf_runtime::FlowNode>, operators: mf_runtime::StreamOperatorFactories, identities: Vec<mf_runtime::NodeIdentity>) -> Result<mf_runtime::PreparedSubgraph, mf_runtime::generated::BindError> {
                        use snafu::ResultExt as _;
                        mf_runtime::PreparedSubgraph::from_stream_plan(#module::prepare(nodes), operators, identities, #interface()).context(mf_runtime::generated::NodeSnafu { definition_id: mf_runtime::DefinitionId::from(#owner) })
                    }
                });
            } else {
                layouts.push(subgraph_stream_tokens(&body_scope, &node.id, streams)?);
                if body.relations.is_callable() {
                    body.definition
                        .outputs
                        .retain(|o| !streams.iter().any(|s| s.name == o.name));
                }
                layouts.push(task_layout_tokens(
                    &body.definition,
                    &body.execution_order,
                    &body_scope,
                    layout.domains.as_ref().expect("finite child domains"),
                ));
            }
            collect_body_layouts(&body, &body_scope, path, body_layouts, layouts)?;
            path.pop();
        }
    }
    Ok(())
}

#[cfg(feature = "codegen")]
fn success_guard_tokens(
    definition: &WorkflowDefinition,
    order: &[DefinitionId],
    scope: &str,
) -> Vec<TokenStream> {
    let incoming = crate::compiler::incoming_dependencies(definition);
    let guards = crate::dependencies::success_conditions(order.iter().map(|id| {
        (
            id.as_str(),
            incoming.get(id.as_str()).cloned().unwrap_or_default(),
        )
    }));
    guards.into_iter().enumerate().filter_map(|(position, conditions)| {
        if conditions.is_empty() { return None; }
        let name = format_ident!("success_guard_{scope}_{position}");
        let target = LitStr::new(order[position].as_str(), Span::call_site());
        let checks = conditions.into_iter().map(|condition| {
            let source = LitStr::new(&condition.source, Span::call_site());
            let clauses = condition.clauses.into_iter().map(|clause| {
                let available = clause.into_iter().map(|(node, port)| {
                    let key = LitStr::new(&mf_runtime::output_id(&node, &port), Span::call_site());
                    quote! { ctx.output_available_key(#key)? }
                });
                quote! { (false #(| #available)*) }
            });
            quote! {
                let active = (|| -> Result<bool, mf_runtime::NodeExecutionError> {
                    if let Some(error) = ctx.cancellation().failure() { return Err(error.into()); }
                    Ok(true #(& #clauses)*)
                })().context(mf_runtime::SuccessDependencySnafu { definition_id: #target, source_node: #source })?;
                if !active { causes.insert(mf_runtime::SkipCause::NodeSuccess { node_success: #source.into() }); }
            }
        });
        Some(quote! {
            fn #name(ctx: &mf_runtime::ExecutionContext) -> Result<std::collections::BTreeSet<mf_runtime::SkipCause>, mf_runtime::WorkflowRunError> {
                use snafu::ResultExt as _;
                let mut causes = std::collections::BTreeSet::new();
                #(#checks)*
                Ok(causes)
            }
        })
    }).collect()
}

#[cfg(feature = "codegen")]
fn task_call_tokens(
    position: usize,
    transfers: &[LitStr],
    invoke: Option<TokenStream>,
) -> TokenStream {
    match invoke {
        Some(invoke) => {
            quote! { flow.execute_generated_position(#position, &[#(#transfers),*], state, #invoke)?; }
        }
        None => quote! { flow.execute_position(#position, state)?; },
    }
}

#[cfg(feature = "codegen")]
fn task_guard_tokens(
    definition: &WorkflowDefinition,
    order: &[DefinitionId],
    scope: &str,
) -> TokenStream {
    let guards = success_guard_tokens(definition, order, scope);
    let bindings: Vec<_> = order.iter().enumerate().filter_map(|(position,id)| {
        if !definition.order_edges.iter().any(|edge| &edge.to_node == id) { return None; }
        let guard = format_ident!("success_guard_{scope}_{position}");
        Some(quote! { (mf_runtime::NodeId::new(#position), std::sync::Arc::new(#guard) as std::sync::Arc<mf_runtime::ExecutionGuard>) })
    }).collect();
    let bind = format_ident!("bind_success_guards_{scope}");
    let binding = (!bindings.is_empty()).then(|| quote! {
        fn #bind(flow:mf_runtime::Flow) -> mf_runtime::Flow { flow.with_input_guards(vec![#(#bindings),*]) }
    });
    quote! { #(#guards)* #binding }
}

#[cfg(feature = "codegen")]
fn stream_program_tokens(
    plan: &mf_runtime::StreamPlan,
    definition: &WorkflowDefinition,
    order: &[DefinitionId],
) -> TokenStream {
    let incoming = crate::compiler::incoming_dependencies(definition);
    let owners = crate::construction::stream_output_messages(plan);
    let executions = domain_tokens(plan.execution_domains());
    let mut functions = Vec::new();
    let inputs: Vec<_> = plan.nodes().iter().enumerate().map(|(index, _)| {
        let name = format_ident!("resolve_stream_input_{index}");
        let mut dependencies = incoming.get(order[index].as_str()).cloned().unwrap_or_default();
        dependencies.sort();
        let dependencies: Vec<_> = dependencies.iter().filter_map(|dependency| {
            let output = dependency.source_output()?;
            let source = LitStr::new(dependency.source_node(), Span::call_site());
            let key = LitStr::new(&mf_runtime::output_id(dependency.source_node(), output), Span::call_site());
            let output = LitStr::new(output, Span::call_site());
            let input = dependency.input().map(|input| {
                let input = LitStr::new(input, Span::call_site());
                quote! { Some(#input) }
            }).unwrap_or_else(|| quote! { None });
            let exclusive = dependency.exclusive();
            Some(quote! { mf_runtime::ExecutionDependency { exclusive: #exclusive, input: #input, source_node: #source, source_output: #output, source_key: std::borrow::Cow::Borrowed(#key) } })
        }).collect();
        functions.push(quote! {
            fn #name(plan: &mf_runtime::StreamPlan, context: &mf_runtime::ExecutionContext, inputs: &mut mf_runtime::Inputs, causes: std::collections::BTreeSet<mf_runtime::SkipCause>) -> Result<mf_runtime::InputResolution, mf_runtime::WorkflowRunError> {
                context.resolve_node_inputs(&plan.nodes()[#index], [#(#dependencies),*], &[], inputs, causes)
            }
        });
        quote! { std::sync::Arc::new(#name) as std::sync::Arc<mf_runtime::StreamInputResolver> }
    }).collect();
    let emissions: Vec<_> = plan.program().emissions.iter().enumerate().map(|(index, route)| {
        let Some(route) = route else { return quote! { None }; };
        let name = format_ident!("initialize_stream_message_{index}");
        let message = route.message;
        let connected = route.connected;
        let terminal = route.terminal;
        functions.push(quote! {
            fn #name(plan: &mf_runtime::StreamPlan, result: mf_runtime::NodeResult) -> Result<mf_runtime::ExecutionContext, mf_runtime::WorkflowRunError> {
                mf_runtime::ExecutionContext::for_message(plan.emission_node(#index), result)
            }
        });
        quote! { Some(mf_runtime::StreamEmissionBinding { message: #message, connected: #connected, terminal: #terminal, initialize: std::sync::Arc::new(#name) }) }
    }).collect();
    let messages: Vec<_> = plan.program().messages.iter().enumerate().map(|(message, program)| {
        let source = match program.source { Some(source) => quote! { Some(#source) }, None => quote! { None } };
        let schedule = &program.schedule;
        let domains = schedule.domains.iter();
        let local = schedule.local.iter().map(|&rank| if rank == usize::MAX { quote! { usize::MAX } } else { quote! { #rank } });
        let initial = schedule.initial_deps.iter();
        let successors = schedule.successors.iter().map(|ranks| { let ranks = ranks.iter(); quote! { vec![#(#ranks),*].into_boxed_slice() } });
        let select = if program.select_outputs.is_some() {
            let name = format_ident!("select_stream_outputs_{message}");
            let outputs = definition.outputs.iter().filter(|output| {
                owners[&(output.node.as_str(), output.port.as_ref())].1 == message
            }).map(|output| {
                let name = LitStr::new(&output.name, Span::call_site());
                let node = LitStr::new(output.node.as_str(), Span::call_site());
                let key = LitStr::new(&mf_runtime::output_id(output.node.as_str(), &output.port), Span::call_site());
                let optional = output.optional;
                quote! {
                    if let Some(value) = context.select_output_keyed(#name, #node, #key, #optional)? { outputs.insert(#name.into(), value); }
                }
            });
            functions.push(quote! {
                fn #name(context: &mut mf_runtime::ExecutionContext) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
                    let mut outputs = mf_runtime::FlowOutputs::new();
                    #(#outputs)*
                    Ok(outputs)
                }
            });
            quote! { Some(std::sync::Arc::new(#name) as std::sync::Arc<mf_runtime::StreamOutputSelector>) }
        } else { quote! { None } };
        quote! { mf_runtime::StreamMessageProgram {
            source: #source,
            schedule: mf_runtime::MessageSchedule { domains: vec![#(#domains),*].into_boxed_slice(), local: vec![#(#local),*].into_boxed_slice(), initial_deps: vec![#(#initial),*].into_boxed_slice(), successors: vec![#(#successors),*].into_boxed_slice() },
            select_outputs: #select,
        } }
    }).collect();
    quote! {
        #(#functions)*
        static STREAM_EXECUTION_DOMAINS: mf_runtime::ExecutionDomains = #executions;
        fn stream_program() -> mf_runtime::StreamProgram {
            mf_runtime::StreamProgram { execution_domains: STREAM_EXECUTION_DOMAINS.clone(), messages: vec![#(#messages),*], emissions: vec![#(#emissions),*], inputs: vec![#(#inputs),*] }
        }
    }
}

#[cfg(feature = "codegen")]
fn stream_guard_tokens(
    definition: &WorkflowDefinition,
    order: &[DefinitionId],
    stream: &mf_runtime::StreamPlan,
) -> TokenStream {
    let guards = success_guard_tokens(definition, order, "stream");
    let mut bindings = Vec::new();
    let mut functions = Vec::new();
    for domain in stream.execution_domains().domains() {
        if stream.nodes()[domain.nodes[0].index()].node.is_none() {
            continue;
        }
        let statements = domain.positions.iter().map(|&position| {
            let causes = if definition.order_edges.iter().any(|edge| edge.to_node == order[position]) {
                let guard = format_ident!("success_guard_stream_{position}");
                quote! { #guard(ctx)? }
            } else { quote! { std::collections::BTreeSet::new() } };
            let resolve = format_ident!("resolve_stream_input_{position}");
            quote! {
                if state.cancellation().failure().is_some() || state.scope_exit_requested() { return Ok(()); }
                plan.execute_resolved_position(#position, state, |_, ctx, inputs| {
                    #resolve(plan, ctx, inputs, #causes)
                })?;
            }
        });
        let id = domain.id;
        let name = format_ident!("execute_stream_domain_{id}");
        functions.push(quote! {
            fn #name(plan: &mf_runtime::StreamPlan, state: &mut mf_runtime::ExecutionContext) -> Result<(), mf_runtime::WorkflowRunError> {
                #(#statements)*
                Ok(())
            }
        });
        bindings.push(quote! { (#id, std::sync::Arc::new(#name) as std::sync::Arc<mf_runtime::GeneratedStreamDomainExecutor>) });
    }
    quote! {
        #(#guards)*
        #(#functions)*
        fn bind_stream_guards(prepared: mf_runtime::PreparedStream) -> mf_runtime::PreparedStream {
            prepared.with_prepared_domains(vec![#(#bindings),*])
        }
    }
}

#[cfg(all(test, feature = "codegen"))]
mod tests {
    extern crate mfn_core as _;

    use super::*;
    use serde_json::json;

    #[test]
    fn generated_origins_use_absolute_paths_and_the_published_root_identity() {
        let definition: WorkflowDefinition = serde_json::from_value(json!({
            "version":"2026-10-28", "execution":{}, "dependencies":{},
            "nodes":[{"id":"outer", "kind":"workflow.call", "config":{
                "flow":"inline", "body":{
                    "version":"2026-10-28", "dependencies":{},
                    "nodes":[{"id":"make", "kind":"builtin.constant", "config":{
                        "type":"flow", "value":{"version":"2026-10-28", "dependencies":{}, "nodes":[]}
                    }}],
                    "outputs":[{"name":"flow", "node":"make", "port":"value"}]
                }
            }}],
            "outputs":[{"name":"flow", "node":"outer", "port":"flow"}]
        }))
        .unwrap();
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let plan = crate::plan_definition(&definition).unwrap();
        let initial_identity = plan.graph.workflow_id().unwrap();
        let artifacts = plan
            .generate_artifacts(&registry, Default::default())
            .unwrap();
        let manifest = &artifacts.manifest;
        let identity = manifest.workflow_id().unwrap();
        assert!(manifest.definition.execution.is_none());
        assert_ne!(initial_identity, identity);
        assert!(artifacts.rust_source.contains(identity.as_str()));
        assert!(
            artifacts
                .rust_source
                .contains("fn mf_compiled_flow_origin(")
        );
        assert_eq!(artifacts.rust_source.matches(".with_origin(").count(), 2);
        let body_key = mf_runtime::ScopePathEntry::binding_key("make", "value");
        let body_key = LitStr::new(&body_key, Span::call_site());
        let expected = quote!("outer".into(), #body_key.into());
        let compact = |source: &str| source.split_whitespace().collect::<String>();
        assert!(compact(&artifacts.rust_source).contains(&compact(&expected.to_string())));
        assert_eq!(
            manifest.description().unwrap().version,
            mf_telemetry::description::WorkflowDescriptionVersion::V2026_11_09,
        );
    }

    #[test]
    fn generated_input_calls_use_ordinary_receivers_and_keep_factory_validation() {
        let definition: WorkflowDefinition = serde_json::from_value(json!({
            "version":"2026-10-28", "dependencies":{},
            "nodes":[
                {"id":"formal", "kind":"builtin.parameters", "config":{
                    "parameters":{"callback":{"type":"flow", "secret":false}}
                }},
                {"id":"apply", "kind":"workflow.call", "config":{"inputs":[], "outputs":[]},
                    "inputs":{"flow":{"from":{"node":"formal", "port":"callback"}}}}
            ]
        }))
        .unwrap();
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let plan = crate::plan_definition(&definition).unwrap();
        let artifacts = plan
            .generate_artifacts(&registry, Default::default())
            .unwrap();
        let manifest = &artifacts.manifest;
        assert!(manifest.descriptor.description.loop_bodies.is_empty());
        let payload = serde_json::to_value(manifest).unwrap();
        assert!(payload.get("graph").is_none());
        assert_eq!(
            manifest.descriptor.input_schema.inputs["formal"]["callback"].value_type,
            mf_runtime::ValueType::Flow
        );
        assert!(!artifacts.rust_source.contains("mf_compiled_flow_origin"));
        assert!(
            artifacts
                .rust_source
                .contains("mf_runtime::generated::instantiate")
        );
        assert!(
            !artifacts
                .rust_source
                .contains("PreparedSubgraph::from_plan")
        );

        let config = serde_json::to_string(&definition.nodes[1].config).unwrap();
        let (registration, config_value) = mf_runtime::generated::configuration(
            &registry,
            "apply",
            crate::CALL_KIND,
            &config,
            &[],
        )
        .unwrap();
        assert!(
            mf_runtime::generated::configuration(
                &registry,
                "apply",
                crate::CALL_KIND,
                &config,
                &["body"]
            )
            .is_err()
        );
        let node = mf_runtime::generated::instantiate(
            registration,
            "apply",
            config_value,
            BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            node.metadata.ports.inputs,
            [crate::PortSpec::new(
                "flow",
                mf_runtime::ValueType::Flow,
                true
            )]
        );

        let mut invalid = definition;
        invalid.nodes[1].config["inputs"] = json!([
            {"name":"flow", "value_type":"flow", "required":true}
        ]);
        // Source resolution does not run the factory's reserved-port check.
        let plan = crate::plan_definition(&invalid).unwrap();
        assert!(matches!(
            plan.generate_artifacts(&registry, Default::default()).unwrap_err(),
            PlanError::Compilation { source }
                if matches!(source.as_ref(), crate::WorkflowCompileError::NodeConstruction {
                    definition_id, source: crate::NodeBuildError::FactoryFailed { .. }, ..
                } if definition_id.as_str() == "apply")
        ));
    }

    #[test]
    fn generated_scope_constructors_follow_relations_not_kinds_or_config() {
        let graph = mf_runtime::CompiledGraph {
            definition: WorkflowDefinition {
                version: mf_runtime::WorkflowDefinitionVersion::V2026_09_26,
                execution: None,
                dependencies: BTreeMap::new(),
                flows: BTreeMap::new(),
                variables: Vec::new(),
                nodes: [
                    ("custom-input", "custom.source"),
                    ("custom-write", "custom.write"),
                    ("custom-exit", "custom.exit"),
                    ("router-write", crate::ASSIGN_KIND),
                    ("ordinary", crate::ASSIGN_KIND),
                ]
                .into_iter()
                .map(|(id, kind)| crate::NodeDefinition {
                    id: id.into(),
                    kind: kind.into(),
                    config: serde_json::Value::Null,
                })
                .collect(),
                edges: Vec::new(),
                control_edges: Vec::new(),
                outputs: Vec::new(),
                order_edges: Vec::new(),
                clusters: Vec::new(),
            },
            execution_order: [
                "custom-input",
                "custom-write",
                "custom-exit",
                "router-write",
                "ordinary",
            ]
            .into_iter()
            .map(DefinitionId::from)
            .collect(),
            relations: mf_runtime::GraphRelations {
                scope: Some(mf_runtime::ScopeRelations {
                    input: "custom-input".into(),
                    values: BTreeMap::from([("item".into(), mf_runtime::ValueType::String)]),
                    state_outputs: BTreeMap::from([("item".into(), "result".into())]),
                    writes: BTreeMap::from([("custom-write".into(), "item".into())]),
                    exits: BTreeSet::from(["custom-exit".into()]),
                }),
                router_writes: BTreeMap::from([(
                    "router-write".into(),
                    mf_runtime::RouterWrite {
                        router: "choose".into(),
                        routes: vec!["first".into()],
                        enables: vec!["first".into()],
                    },
                )]),
                ..Default::default()
            },
            children: BTreeMap::new(),
        };
        let (preparations, _, _) = crate::fixed::scope(
            &graph,
            "child",
            &["owner".into()],
            &BTreeMap::new(),
            false,
            &Default::default(),
            None,
        )
        .unwrap();
        let source = quote! { #(#preparations)* }.to_string();
        assert!(source.contains("prepared_scope_source"));
        assert!(source.contains("prepared_loop_assign"));
        assert!(source.contains("prepared_loop_exit"));
        assert!(source.contains("prepared_router_assign"));
        assert_eq!(source.matches("generated :: instantiate").count(), 1);
        assert!(source.contains("\"ordinary\""));
        assert!(!source.contains("iteration_input_flow_node"));
        assert!(!source.contains("from_json"));
    }
}
