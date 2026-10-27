pub mod conversation {
    use crate::{ChatError, error::ConversationDefinitionSnafu};
    use mf_compiler::CompiledWorkflow;
    use mf_runtime::NodeRegistry;
    use snafu::ResultExt;
    use std::sync::Mutex;

    pub fn plan() -> Result<CompiledWorkflow, ChatError> {
        static PLAN: Mutex<Option<CompiledWorkflow>> = Mutex::new(None);
        let mut plan = PLAN.lock().expect("conversation plan was not poisoned");
        if plan.is_none() {
            // Resolve the application's own built-in Flow after its provider has been linked.
            let definition = mf_runtime::WorkflowDefinition::from_json(include_str!(
                "../workflows/conversation.json"
            ))
            .context(ConversationDefinitionSnafu)?;
            *plan = Some(mf_compiler::compile_definition(
                &definition,
                &NodeRegistry::from_inventory()?,
            )?);
        }
        Ok(plan
            .as_ref()
            .expect("conversation plan was prepared")
            .clone())
    }

    pub fn manifest(registry: &NodeRegistry) -> Result<mf_runtime::WorkflowManifest, ChatError> {
        Ok(plan()?.manifest(registry)?)
    }

    pub fn prepare_stream(
        registry: &NodeRegistry,
    ) -> Result<mf_runtime::PreparedStream, ChatError> {
        Ok(mf_compiler::instantiate_stream(&plan()?, registry)?)
    }

    pub fn prepare_workflow(
        registry: &NodeRegistry,
        observation: Option<&mut mf_runtime::RunObservation>,
    ) -> Result<mf_runtime::Flow, ChatError> {
        Ok(mf_compiler::instantiate_compiled_with_observation(
            &plan()?,
            registry,
            observation,
        )?)
    }
}

pub fn title_definition() -> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError>
{
    mf_runtime::WorkflowDefinition::from_json(include_str!(concat!(env!("OUT_DIR"), "/title.json")))
}

mf_runtime::register_builtin_flow!("chat-title", title_definition);
