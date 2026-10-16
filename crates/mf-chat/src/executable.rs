//! Parent-owned chat invocation state and executable contracts.

use crate::{ChatError, Completion, Invocation, ModelIdentity, error::*, turn};
use mf_runtime::{
    ConfigTemplate, NodeValue, WorkflowArguments, WorkflowInputSchema, WorkflowManifest,
};
use sha2::{Digest, Sha256};
use snafu::{OptionExt, ResultExt, ensure};
use std::collections::BTreeMap;

mod recovery;
pub use recovery::{CompletionState, ConversationExit, MAX_RECOVERY_BYTES, ReconciledCompletion};

#[derive(Clone, Debug)]
pub struct ChatContract {
    pub node: String,
    pub model: serde_json::Value,
    pub config_id: String,
    pub parameters: WorkflowInputSchema,
    inputs: BTreeMap<String, String>,
    pub parameterized: bool,
}
impl ChatContract {
    pub fn from_manifest(manifest: &WorkflowManifest) -> Result<Self, ChatError> {
        manifest.validate()?;
        let graph = manifest.description()?;
        let models: Vec<_> = graph
            .nodes
            .iter()
            .filter(|node| node.kind == mfn_llm::KIND)
            .collect();
        let parameters: Vec<_> = graph
            .nodes
            .iter()
            .filter(|node| node.kind == "builtin.parameters")
            .collect();
        ensure!(
            !graph.is_streaming()
                && graph.loop_bodies.is_empty()
                && models.len() == 1
                && parameters.len() <= 1
                && graph.nodes.len() == models.len() + parameters.len()
                && graph.control_edges.is_empty(),
            InvalidSnafu {
                field: "chat executable",
                reason: "requires one finite LLM node and at most one initial parameter node"
            }
        );
        ensure!(
            manifest.input_schema.stdin.is_empty(),
            InvalidSnafu {
                field: "chat executable",
                reason: "must not require stdin"
            }
        );
        let node = models[0];
        let source = parameters.first().copied().unwrap_or(node);
        let ports = manifest
            .input_schema
            .inputs
            .get(&source.id)
            .context(InvalidSnafu {
                field: "chat executable",
                reason: "requires conversation startup inputs",
            })?;
        ensure!(
            manifest.input_schema.inputs.len() == 1,
            InvalidSnafu {
                field: "chat executable",
                reason: "requires a single startup source"
            }
        );
        let parameterized = !parameters.is_empty();
        let mut inputs = BTreeMap::new();
        let mut configuration = manifest.input_schema.clone();
        for expected in mfn_llm::LlmInputs::ports() {
            let name = if parameterized {
                let edges: Vec<_> = graph
                    .data_edges
                    .iter()
                    .filter(|edge| edge.to_node == node.id && edge.to_input == expected.name)
                    .collect();
                ensure!(
                    edges.len() == 1 && edges[0].from_node == source.id,
                    InvalidSnafu {
                        field: "chat executable",
                        reason: "parameter outputs must feed prompt, system, and history directly"
                    }
                );
                edges[0].from_output.as_str()
            } else {
                expected.name.as_ref()
            };
            let port = ports.get(name).context(InvalidSnafu {
                field: "chat executable",
                reason: "missing a conversation input",
            })?;
            ensure!(
                port.value_type == expected.value_type
                    && (parameterized || port.required == expected.required),
                InvalidSnafu {
                    field: "chat executable",
                    reason: "requires fixed prompt, system, and history types"
                }
            );
            ensure!(
                configuration
                    .inputs
                    .get_mut(&source.id)
                    .unwrap()
                    .remove(name)
                    .is_some(),
                InvalidSnafu {
                    field: "chat executable",
                    reason: "conversation ports must use distinct startup parameters"
                }
            );
            inputs.insert(
                expected.name.to_string(),
                mf_runtime::output_id(&source.id, name),
            );
        }
        ensure!(
            graph.data_edges.len() == if parameterized { 3 } else { 0 },
            InvalidSnafu {
                field: "chat executable",
                reason: "only conversation input edges are supported"
            }
        );
        ensure!(
            parameterized || ports.len() == inputs.len(),
            InvalidSnafu {
                field: "chat executable",
                reason: "requires fixed LLM startup inputs"
            }
        );
        if parameterized {
            let expected = mfn_llm::LlmOutputs::ports();
            ensure!(
                manifest.definition.outputs.len() == expected.len()
                    && expected
                        .iter()
                        .all(
                            |port| manifest.definition.outputs.iter().any(|output| output.name
                                == port.name
                                && output.node.as_str() == node.id
                                && output.port == port.name
                                && output.optional != port.required)
                        ),
                InvalidSnafu {
                    field: "chat outputs",
                    reason: "requires all LLM outputs under their fixed names"
                }
            );
        }
        let config = &manifest.node_definition(&node.id).unwrap().config;
        let model = config
            .get("model")
            .context(InvalidSnafu {
                field: "model identity",
                reason: "requires the configured model or template",
            })?
            .clone();
        let template = ConfigTemplate::parse(model.clone())?;
        for reference in template.references("model") {
            ensure!(
                configuration.parameter(&reference.output).is_some(),
                InvalidSnafu {
                    field: "model identity",
                    reason: "model references must name configuration parameters"
                }
            );
        }
        let config_id: String = Sha256::digest(serde_json::to_vec(config).context(JsonSnafu)?)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let contract = Self {
            node: node.id.clone(),
            model,
            config_id,
            parameters: configuration,
            inputs,
            parameterized,
        };
        if !parameterized {
            contract.model_identity("default", &WorkflowArguments::default())?;
        }
        Ok(contract)
    }

    pub fn model_identity(
        &self,
        profile: &str,
        parameters: &WorkflowArguments,
    ) -> Result<ModelIdentity, ChatError> {
        let template = ConfigTemplate::parse(self.model.clone())?;
        ensure!(
            template
                .references("model")
                .iter()
                .all(|reference| parameters.0.contains_key(&reference.output)),
            InvalidSnafu {
                field: "model identity",
                reason: "model parameters must be resolved before admission"
            }
        );
        let model = template.evaluate(&|output| Ok(parameters.0[output].clone()))?;
        ensure!(
            !model.contains_secret(),
            InvalidSnafu {
                field: "model identity",
                reason: "the accounting model must use public parameters"
            }
        );
        let model = model
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .context(InvalidSnafu {
                field: "model identity",
                reason: "requires a nonblank resolved model name",
            })?
            .to_owned();
        let config_id = if self.parameterized {
            let public: BTreeMap<_, _> = parameters
                .0
                .iter()
                .filter(|(_, value)| !value.contains_secret())
                .collect();
            Sha256::digest(serde_json::to_vec(&(&self.config_id, public)).context(JsonSnafu)?)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        } else {
            self.config_id.clone()
        };
        Ok(ModelIdentity {
            profile: profile.into(),
            model,
            config_id,
        })
    }

    pub fn arguments(
        &self,
        mut input: mfn_llm::LlmInputs,
        parameters: &WorkflowArguments,
    ) -> Result<WorkflowArguments, ChatError> {
        if self.parameterized {
            input.system.get_or_insert_default();
            input.history.get_or_insert_default();
        }
        let mut arguments = parameters.clone();
        for (name, value) in input.into_values()? {
            arguments.0.insert(self.inputs[&name].clone(), value);
        }
        Ok(arguments)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnExit {
    Failed,
    Cancelled,
    Interrupted,
}
impl TurnExit {
    pub fn state(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Clone, Debug)]
pub enum InvocationState {
    NotBegun,
    Running,
    Committed(Completion),
    Finished(crate::Turn),
}

impl crate::ChatDatabase {
    pub fn inspect_invocation(
        &self,
        invocation: &Invocation,
    ) -> Result<InvocationState, ChatError> {
        let turn = match self.turn(&invocation.turn_id) {
            Ok(turn) => turn,
            Err(error) => {
                let mut source: Option<&dyn std::error::Error> = Some(&error);
                while let Some(cause) = source {
                    if matches!(cause.downcast_ref::<ChatError>(), Some(ChatError::TurnMissing { id }) if id == &invocation.turn_id)
                    {
                        return Ok(InvocationState::NotBegun);
                    }
                    source = cause.source();
                }
                return Err(error);
            }
        };
        let call = self.call(&invocation.call_id)?;
        ensure!(
            turn.session_id == invocation.session_id
                && call.session_id == invocation.session_id
                && call.turn_id.as_deref() == Some(invocation.turn_id.as_str())
                && call.purpose == "conversation"
                && call.state == turn.state,
            ConflictSnafu {
                id: &invocation.call_id
            }
        );
        match turn.state.as_str() {
            "running" => Ok(InvocationState::Running),
            "succeeded" => Ok(InvocationState::Committed(Completion {
                invocation: invocation.clone(),
                reply: turn::saved_reply(&call)?,
            })),
            "failed" | "cancelled" | "interrupted" => Ok(InvocationState::Finished(turn)),
            _ => ConflictSnafu {
                id: &invocation.call_id,
            }
            .fail(),
        }
    }

    pub fn finalize_invocation(
        &self,
        invocation: Invocation,
        outcome: TurnExit,
        summary: &str,
    ) -> Result<InvocationState, ChatError> {
        match self.inspect_invocation(&invocation)? {
            InvocationState::Running => {
                self.fail_turn(invocation.clone(), outcome.state(), summary)?;
                self.inspect_invocation(&invocation)
            }
            state => Ok(state),
        }
    }

    pub fn retry_prompt(&self, turn_id: &str) -> Result<(crate::Turn, String), ChatError> {
        let original: turn::TurnRequestOutputs = crate::operation::execute(
            self.path(),
            "mf_chat.turn.request",
            turn::RecordInputs {
                database_path: String::new(),
                id: turn_id.into(),
            },
        )?;
        ensure!(
            matches!(
                original.turn.state.as_str(),
                "failed" | "cancelled" | "interrupted"
            ),
            InvalidSnafu {
                field: "retry_of",
                reason: "must identify an unsuccessful turn"
            }
        );
        Ok((original.turn, original.prompt))
    }
}
