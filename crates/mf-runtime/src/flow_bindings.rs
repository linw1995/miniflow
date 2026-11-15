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
    /// Input carrying a statically bounded Flow map, if this node consumes one.
    pub invocation: Option<String>,
}

impl FlowBindings {
    /// Declares a static branch receiver without forwarding any Flow outputs.
    pub fn invoker(input: impl Into<String>) -> Self {
        Self {
            invocation: Some(input.into()),
            ..Self::default()
        }
    }
}
