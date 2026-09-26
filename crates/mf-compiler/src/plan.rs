use crate::definition::{DefinitionId, WorkflowDefinition};
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
#[snafu(visibility(pub))]
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
}

impl CompiledWorkflow {
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

        let mut preparations: Vec<TokenStream> = Vec::new();
        let mut node_statements: Vec<TokenStream> = Vec::new();
        for (index, definition_id) in self.execution_order.iter().enumerate() {
            let node = nodes_by_id[definition_id];
            let config_json = serde_json::to_string(&node.config).context(SerializeSnafu)?;
            let config_lit = LitStr::new(&config_json, Span::call_site());
            let node_ident = format_ident!("node_{index}");
            let id_lit = LitStr::new(node.id.as_str(), Span::call_site());
            let kind_lit = LitStr::new(&node.kind, Span::call_site());
            preparations.push(quote! {
                let #node_ident = mf_runtime::instantiate_node_with_metadata(registry, #id_lit, #kind_lit, #config_lit)?;
            });
            let mut bindings: Vec<TokenStream> = Vec::new();
            for edge in self
                .definition
                .edges
                .iter()
                .filter(|edge| edge.to_node == *definition_id)
            {
                let source = LitStr::new(edge.from_node.as_str(), Span::call_site());
                let port = LitStr::new(&edge.from_output, Span::call_site());
                let input = LitStr::new(&edge.to_input, Span::call_site());
                bindings.push(quote! { mf_runtime::ExecutionDependency { input: Some(#input), source_node: #source, source_output: #port } });
            }
            for edge in self
                .definition
                .control_edges
                .iter()
                .filter(|edge| edge.to_node == *definition_id)
            {
                let source = LitStr::new(edge.from_node.as_str(), Span::call_site());
                let port = LitStr::new(&edge.from_output, Span::call_site());
                bindings.push(quote! { mf_runtime::ExecutionDependency { input: None, source_node: #source, source_output: #port } });
            }
            node_statements.push(quote! {
                mf_runtime::execute_node_in_context(&#node_ident, &[#(#bindings),*], &mut state)?;
            });
        }

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
                if let Some(value) = mf_runtime::select_context_output(&state, #output_name, #node_id, #port, #optional)? {
                    workflow_outputs.insert(#output_name.to_owned(), value);
                }
            });
        }

        let generated = quote! {
            pub fn run_workflow(
                registry: &mf_runtime::NodeRegistry,
            ) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
                let mut state = mf_runtime::ExecutionContext::default();
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
