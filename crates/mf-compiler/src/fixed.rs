//! Standalone initialization from validated compiler facts. No graph compiler is linked at startup.

use crate::{DefinitionId, PlanError};
use mf_runtime::{CompiledGraph, ScopeInstruction};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use snafu::ResultExt;
use std::collections::BTreeMap;

pub fn scope(
    graph: &CompiledGraph,
    name: &str,
    path: &[String],
    constructors: &BTreeMap<DefinitionId, (TokenStream, TokenStream)>,
    bind_flow: bool,
    error_override: Option<&TokenStream>,
) -> Result<(Vec<TokenStream>, Vec<syn::Ident>, syn::Ident), PlanError> {
    let mut preparation = Vec::new();
    let mut nodes = Vec::new();
    let flow = format_ident!("flow_{name}");
    let path_json = serde_json::to_string(path).expect("scope paths serialize");
    for (index, id) in graph.execution_order.iter().enumerate() {
        let node = graph
            .definition
            .nodes
            .iter()
            .find(|node| &node.id == id)
            .expect("validated node");
        let physical_id = id.as_str();
        let kind = &node.kind;
        let node_name = format_ident!("node_{name}_{index}");
        let binder = format_ident!("bind_node_{name}_{index}");
        let report = error_override.cloned().unwrap_or_else(|| if path.is_empty() {
            quote! { if let Some(observation) = observation.as_deref_mut() { observation.preparation_failed(#physical_id, error.to_string()); } }
        } else {
            quote! { if let Some(observation) = observation.as_deref_mut() { observation.preparation_failed_unattributed(format!("child scope {} node `{}`: {}", #path_json, #physical_id, error)); } }
        });
        if let Some((constructor, _)) = constructors.get(id) {
            let handle = format_ident!("typed_root_{index}");
            preparation.push(quote! {
                let #handle = #constructor.inspect_err(|error| { #report })?;
                let #node_name = #binder(mf_runtime::FlowNode::new(#physical_id, #handle.prepared()))
                    .inspect_err(|error| { #report })?;
            });
            nodes.push(node_name);
            continue;
        }
        let instruction = match graph.relations.instruction(id) {
            Some(ScopeInstruction::RootInput) => {
                let types =
                    crate::literals::value_types(&graph.relations.root.as_ref().unwrap().values);
                Some(quote!(mf_runtime::prepared_root_source(#physical_id, &#types)))
            }
            Some(ScopeInstruction::RootWrite(variable)) => {
                let ty = crate::literals::value_type(
                    &graph.relations.root.as_ref().unwrap().values[variable],
                );
                Some(quote!(mf_runtime::prepared_root_assign(#physical_id, #variable, #ty)))
            }
            Some(ScopeInstruction::Input) => {
                let types =
                    crate::literals::value_types(&graph.relations.scope.as_ref().unwrap().values);
                Some(quote!(mf_runtime::prepared_scope_source(#physical_id, &#types)))
            }
            Some(ScopeInstruction::Write(variable)) => {
                let ty = crate::literals::value_type(
                    &graph.relations.scope.as_ref().unwrap().values[variable],
                );
                Some(quote!(mf_runtime::prepared_loop_assign(#physical_id, #variable, #ty)))
            }
            Some(ScopeInstruction::Exit) => {
                Some(quote!(mf_runtime::prepared_loop_exit(#physical_id)))
            }
            Some(ScopeInstruction::RouterWrite(write)) => {
                let router = write.router.as_str();
                let routes = &write.routes;
                let enables = &write.enables;
                Some(
                    quote!(mf_runtime::prepared_router_assign(#physical_id, #router,
                    vec![#(#routes.into()),*], vec![#(#enables.into()),*])),
                )
            }
            None => None,
        };
        let construct = if let Some(instruction) = instruction {
            instruction
        } else {
            let config =
                serde_json::to_string(&node.config).expect("JSON configuration serializes");
            let mut bodies = Vec::new();
            let mut entries = Vec::new();
            let children = graph.children.get(id);
            let bindings: Vec<_> = children
                .into_iter()
                .flatten()
                .map(|(binding, _)| binding.as_str())
                .collect();
            for (binding_index, (binding, child)) in children.into_iter().flatten().enumerate() {
                let child_name =
                    crate::plan::child_scope_name(name, index, binding_index, node, binding, child);
                let mut child_path = path.to_vec();
                child_path.push(child.child_scope_key(node, binding));
                let override_report = child
                    .relations
                    .scope
                    .as_ref()
                    .is_some_and(|frame| !frame.is_stateful())
                    .then(|| report.clone());
                let (steps, _, child_flow) = scope(
                    child,
                    &child_name,
                    &child_path,
                    &BTreeMap::new(),
                    true,
                    override_report.as_ref(),
                )?;
                let exports = format_ident!("bind_stream_outputs_{child_name}");
                let body_name = format_ident!("body_{child_name}");
                let interface = format_ident!("interface_{child_name}");
                let relations = crate::literals::relations(&child.relations);
                let identities = child.definition.nodes.iter().filter(|node| child.relations.scope.as_ref().is_none_or(|frame| frame.input != node.id)).map(|node| {
                    let id = node.id.as_str(); let kind = &node.kind;
                    quote!(mf_runtime::NodeIdentity { id: #id.into(), kind: #kind.into(), path: Vec::new() })
                });
                let origin_path = &child_path;
                bodies.push(quote! {
                    let #body_name = (|| -> Result<mf_runtime::PreparedSubgraph, mf_runtime::generated::BindError> {
                        #(#steps)*
                        let child_flow = #exports(#child_flow)?;
                        Ok(mf_runtime::PreparedSubgraph::from_plan(child_flow, vec![#(#identities),*], #interface())
                            .with_relations(#relations)
                            .with_origin(mf_compiled_flow_origin(vec![#(#origin_path.into()),*])?))
                    })().context(mf_runtime::generated::ChildSnafu { definition_id: mf_runtime::DefinitionId::from(#physical_id) })?;
                });
                entries.push(quote!((#binding.into(), #body_name)));
            }
            quote! {{
                let (registration, config) = mf_runtime::generated::configuration(registry, #physical_id, #kind, #config, &[#(#bindings),*])?;
                #(#bodies)*
                mf_runtime::generated::instantiate(registration, #physical_id, config,
                    std::collections::BTreeMap::from([#(#entries),*]))?
            }}
        };
        preparation.push(quote! {
            let #node_name = (|| -> Result<mf_runtime::FlowNode, mf_runtime::generated::BindError> {
                #binder(#construct)
            })().inspect_err(|error| { #report })?;
        });
        nodes.push(node_name);
    }
    if bind_flow {
        let plan = format_ident!("FLOW_PLAN_{}", name.to_uppercase());
        let schema = format_ident!("input_schema_{name}");
        let mutable = (graph.relations.root.is_some() || !graph.definition.order_edges.is_empty())
            .then(|| quote!(mut));
        preparation.push(quote! {
            let #mutable #flow = mf_runtime::Flow::from_plan(
                vec![#(mf_runtime::generated::into_task(#nodes)?),*], #plan.clone(), #schema(),
            );
        });
        if let Some(root) = &graph.relations.root {
            let root = crate::literals::root_scope(root);
            preparation.push(quote!(#flow = #flow.with_root_scope(Some(#root));));
        }
        if !graph.definition.order_edges.is_empty() {
            let guards = format_ident!("bind_success_guards_{name}");
            preparation.push(quote!(#flow = #guards(#flow);));
        }
    }
    Ok((preparation, nodes, flow))
}

pub fn stream_artifacts(
    workflow: &crate::CompiledWorkflow,
) -> Result<crate::GeneratedWorkflowArtifacts, PlanError> {
    let (preparation, nodes, _) = scope(
        &workflow.graph,
        "stream",
        &[],
        &BTreeMap::new(),
        false,
        None,
    )?;
    let limits = &workflow
        .graph
        .definition
        .execution
        .as_ref()
        .expect("resolved stream mode")
        .limits;
    let pending = limits.max_pending_messages;
    let workers = limits.workers;
    let source = quote! {
        include!(concat!(env!("OUT_DIR"), "/flow-plans.rs"));
        pub fn prepare_stream(registry: &mf_runtime::NodeRegistry) -> Result<mf_runtime::PreparedStream, mf_runtime::generated::BindError> {
            #[allow(unused_imports)]
            use snafu::ResultExt as _;
            let mut observation: Option<&mut mf_runtime::RunObservation> = None;
            let _ = &mut observation;
            let _ = registry;
            #(#preparation)*
            let execution = mf_runtime::StreamExecution { limits: mf_runtime::StreamLimits {
                max_pending_messages: #pending, workers: #workers,
            }};
            Ok(bind_stream_guards(mf_runtime::PreparedStream::from_plan(
                execution, vec![#(#nodes),*], stream_program(), input_schema_stream(),
            )))
        }
    };
    let syntax = syn::parse2(source).context(crate::plan::GeneratedSyntaxSnafu)?;
    Ok(crate::GeneratedWorkflowArtifacts {
        rust_source: prettyplease::unparse(&syntax),
        plan_json: workflow.to_json()?,
    })
}

pub fn bindings(name: &str, layout: &crate::compiler::ScopeLayout) -> TokenStream {
    let bindings = layout.declarations.iter().zip(&layout.contracts).zip(&layout.kinds).enumerate().map(|(index, ((declaration, contract), kind))| {
        let bind = format_ident!("bind_node_{name}_{index}");
        let declared = crate::literals::node_contract(declaration);
        let resolved = crate::literals::node_contract(contract);
        let kind = format_ident!("{kind:?}");
        let success = layout.success_dependencies[index];
        quote! {
            fn #bind(node: mf_runtime::FlowNode) -> Result<mf_runtime::FlowNode, mf_runtime::generated::BindError> {
                mf_runtime::generated::bind(node, &#declared, &#resolved, mf_runtime::generated::ExecutorKind::#kind, #success)
            }
        }
    });
    let schema = crate::literals::input_schema(&layout.input_schema);
    let schema_name = format_ident!("input_schema_{name}");
    let interface = layout.interface.as_ref().map(|interface| {
        let name = format_ident!("interface_{name}");
        let value = crate::literals::subgraph_interface(interface);
        quote!(fn #name() -> mf_runtime::SubgraphInterface { #value })
    });
    quote! { #(#bindings)* fn #schema_name() -> mf_runtime::WorkflowInputSchema { #schema } #interface }
}
