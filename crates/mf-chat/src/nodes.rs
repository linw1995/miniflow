use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EmptyConfig {}

macro_rules! task_registration {
    ($kind:literal, $node:expr) => {
        mf_runtime::NodeRegistration {
            kind: $kind,
            factory: mf_runtime::NodeFactory::Plain(|config| {
                let _: crate::nodes::EmptyConfig = mf_runtime::deserialize_config(config)?;
                mf_runtime::PreparedNode::typed_task($node, mf_runtime::NodeMetadata::default())
            }),
        }
    };
}

macro_rules! register_task {
    ($kind:literal, $node:expr) => {
        inventory::submit! { task_registration!($kind, $node) }
    };
}

pub use crate::session::{
    CreateSessionInputs, ListSessionsInputs, MessagePageOutputs, MessagesInputs,
    RenameSessionInputs, SessionInputs, SessionOutputs, SessionPageOutputs,
};

pub use crate::context::{ModelContextInputs, ModelContextOutputs};

pub use crate::turn::{
    BeginTurnInputs, CallOutputs, Changed, CompleteTurnInputs, CompletionOutputs, FailCallInputs,
    FailTurnInputs, RecordInputs, RecoveryCounts, RecoveryInputs, RecoveryOutputs, TurnOutputs,
    TurnStarted,
};
