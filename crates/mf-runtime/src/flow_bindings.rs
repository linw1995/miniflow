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

impl FlowBindings {
    /// Declares a static branch receiver without forwarding any Flow outputs.
    pub fn invoker(input: impl Into<String>) -> Self {
        Self {
            invocation: Some(FlowInvocationBinding {
                input: input.into(),
            }),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_metadata_preserves_the_input_without_claiming_exports() {
        let bindings = FlowBindings::invoker("tools");
        assert!(bindings.outputs.is_empty());
        assert_eq!(bindings.clone().invocation.unwrap().input, "tools");
        assert!(FlowBindings::default().invocation.is_none());
    }
}
