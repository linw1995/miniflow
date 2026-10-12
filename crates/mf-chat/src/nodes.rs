pub use crate::model::{ModelRequest, ModelResponse};
use mf_runtime::{ExecutionContext, NodeValue, TypedNodeResult, TypedTaskNode};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EmptyConfig {}

macro_rules! register_task {
    ($kind:literal, $node:expr) => {
        inventory::submit! {
            mf_runtime::NodeRegistration {
                kind: $kind,
                factory: mf_runtime::NodeFactory::Plain(|config| {
                    let _: crate::nodes::EmptyConfig = mf_runtime::deserialize_config(config)?;
                    mf_runtime::PreparedNode::typed_task($node, mf_runtime::NodeMetadata::default())
                })
            }
        }
    };
}

macro_rules! register_operation {
    ($kind:literal, $name:ident, $input:ty, $output:ty, $execute:path) => {
        struct $name;
        impl mf_runtime::TypedTaskNode for $name {
            type Input = $input;
            type Output = $output;
            fn execute(
                &self,
                input: Self::Input,
                _: &mut mf_runtime::ExecutionContext,
            ) -> Result<mf_runtime::TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError>
            {
                Ok($execute(input)?.into())
            }
        }
        register_task!($kind, $name);
    };
}

pub use crate::session::{
    CreateSessionInputs, HistoryInputs, HistoryOutputs, ListSessionsInputs, MessagePageOutputs,
    MessagesInputs, RenameSessionInputs, SessionInputs, SessionOutputs, SessionPageOutputs,
};

pub use crate::turn::{
    BeginTurnInputs, CallOutputs, Changed, CompleteTurnInputs, CompletionOutputs, FailCallInputs,
    FailTurnInputs, RecordInputs, RecoveryCounts, RecoveryInputs, RecoveryOutputs, TurnOutputs,
    TurnStarted,
};

#[derive(Clone, Debug, NodeValue)]
pub struct InitializeInputs {
    pub database_path: String,
}

#[derive(Clone, Debug, NodeValue)]
pub struct DatabaseLocation {
    pub database_path: String,
}

pub struct Initialize;

impl TypedTaskNode for Initialize {
    type Input = InitializeInputs;
    type Output = DatabaseLocation;

    fn execute(
        &self,
        input: Self::Input,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
        crate::storage::initialize(std::path::Path::new(&input.database_path))?;
        Ok(DatabaseLocation {
            database_path: input.database_path,
        }
        .into())
    }
}

register_task!("mf_chat.storage.initialize", Initialize);
