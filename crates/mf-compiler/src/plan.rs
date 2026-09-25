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
    pub config_files: BTreeMap<String, String>,
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

        for edge in &self.definition.edges {
            let Some(&source_index) = indices.get(&edge.from_node) else {
                return UnknownNodeSnafu {
                    definition_id: edge.from_node.clone(),
                }
                .fail();
            };
            let Some(&target_index) = indices.get(&edge.to_node) else {
                return UnknownNodeSnafu {
                    definition_id: edge.to_node.clone(),
                }
                .fail();
            };
            if source_index >= target_index {
                return InvalidEdgeOrderSnafu {
                    from_node: edge.from_node.clone(),
                    to_node: edge.to_node.clone(),
                }
                .fail();
            }
        }

        let mut node_statements: Vec<TokenStream> = Vec::with_capacity(self.execution_order.len());
        let mut config_files = BTreeMap::new();
        for (index, definition_id) in self.execution_order.iter().enumerate() {
            let node = nodes_by_id[definition_id];
            let config_file = format!("config_{index}.json");
            let config_json = serde_json::to_string(&node.config).context(SerializeSnafu)?;
            let config_file_lit = LitStr::new(&config_file, Span::call_site());
            config_files.insert(config_file, config_json);

            let node_ident = format_ident!("node_{index}");
            let inputs_ident = format_ident!("inputs_{index}");
            let outputs_ident = format_ident!("_outputs_{index}");
            let definition_id_lit = LitStr::new(node.id.as_str(), Span::call_site());
            let kind_lit = LitStr::new(&node.kind, Span::call_site());

            let incoming: Vec<_> = self
                .definition
                .edges
                .iter()
                .filter(|edge| edge.to_node == *definition_id)
                .collect();
            let inputs_binding = if incoming.is_empty() {
                quote! { let #inputs_ident = mf_runtime::Inputs::new(); }
            } else {
                quote! { let mut #inputs_ident = mf_runtime::Inputs::new(); }
            };
            let input_statements: Vec<TokenStream> = incoming
                .into_iter()
                .map(|edge| {
                    let source_index = indices[&edge.from_node];
                    let source_outputs = format_ident!("_outputs_{source_index}");
                    let input_name = LitStr::new(&edge.to_input, Span::call_site());
                    let source_id = LitStr::new(edge.from_node.as_str(), Span::call_site());
                    let source_port = LitStr::new(&edge.from_output, Span::call_site());
                    quote! {
                        #inputs_ident.insert(
                            #input_name.to_owned(),
                            mf_runtime::required_output(&#source_outputs, #source_id, #source_port)?,
                        );
                    }
                })
                .collect();
            node_statements.push(quote! {
                let #node_ident = mf_runtime::instantiate_node(
                    registry,
                    #definition_id_lit,
                    #kind_lit,
                    include_str!(#config_file_lit),
                )?;
                #inputs_binding
                #(#input_statements)*
                let #outputs_ident = mf_runtime::execute_node(
                    #node_ident.as_ref(),
                    #inputs_ident,
                    #definition_id_lit,
                )?;
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
            let Some(&node_index) = indices.get(&output.node) else {
                return UnknownNodeSnafu {
                    definition_id: output.node.clone(),
                }
                .fail();
            };
            let node_outputs = format_ident!("_outputs_{node_index}");
            let output_name = LitStr::new(&output.name, Span::call_site());
            let node_id = LitStr::new(output.node.as_str(), Span::call_site());
            let port = LitStr::new(&output.port, Span::call_site());
            output_statements.push(quote! {
                workflow_outputs.insert(
                    #output_name.to_owned(),
                    mf_runtime::required_output(&#node_outputs, #node_id, #port)?,
                );
            });
        }

        let generated = quote! {
            pub fn run_workflow(
                registry: &mf_runtime::NodeRegistry,
            ) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
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
            config_files,
        })
    }
}
