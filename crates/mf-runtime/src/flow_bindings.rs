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
    /// A receiving node whose static branches are bound from an incoming Flow map.
    pub invocation: Option<FlowInvocationBinding>,
}

#[derive(Clone, Debug)]
pub struct FlowInvocationBinding {
    pub input: String,
}
