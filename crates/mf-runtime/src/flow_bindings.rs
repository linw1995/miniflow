use serde_json::Value;
use std::collections::BTreeMap;

/// Provider-declared capability provenance, independent of port type inference.
#[derive(Clone, Debug)]
pub enum FlowBindingSource {
    Child(String),
    Input(String),
    Object(BTreeMap<String, String>),
}

#[derive(Clone, Debug, Default)]
pub struct FlowBindings {
    pub outputs: BTreeMap<String, FlowBindingSource>,
    /// A compile-time dispatch template, expanded into gates and static Calls.
    pub invocation: Option<FlowInvocationBinding>,
}

#[derive(Clone, Debug)]
pub struct FlowInvocationBinding {
    pub input: String,
}

/// The strict authoring form of a compile-time dispatch template.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowDispatchConfig {
    pub dispatch: bool,
}

impl FlowDispatchConfig {
    pub fn bindings(config: &Value) -> FlowBindings {
        match serde_json::from_value::<Self>(config.clone()) {
            Ok(Self { dispatch: true, .. }) => FlowBindings {
                invocation: Some(FlowInvocationBinding {
                    input: "flows".into(),
                }),
                ..Default::default()
            },
            _ => FlowBindings::default(),
        }
    }
}
