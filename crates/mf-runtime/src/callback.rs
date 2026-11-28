//! Invocation-local implementations for explicitly declared, finite host call sites.

use crate::{
    CompiledGraph, FlowOutputs, NodeValues, PortSpec, StreamCancellation, WorkflowArguments,
};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu, ensure};
use std::{collections::BTreeMap, error::Error, fmt, sync::Arc};

pub type CallbackSites = BTreeMap<String, Vec<String>>;
pub type HostCallbacks = BTreeMap<String, HostCallback>;

/// A callback is opt-in at both ends. Never export capabilities from a tool value,
/// routed copy, or another movable child. Repeated names have no implicit winner.
pub fn callback_sites(graph: &CompiledGraph) -> CallbackSites {
    fn visit(
        graph: &CompiledGraph,
        path: &mut Vec<String>,
        sites: &mut BTreeMap<String, Option<Vec<String>>>,
    ) {
        for node in &graph.definition.nodes {
            for (binding, child) in graph.children.get(&node.id).into_iter().flatten() {
                if child.relations.static_binding
                    || (child.relations.host.is_none() && child.relations.scope.is_none())
                {
                    continue;
                }
                path.push(child.child_scope_key(node, binding));
                if let Some(name) = child.relations.callback_name() {
                    sites
                        .entry(name.into())
                        .and_modify(|path| *path = None)
                        .or_insert_with(|| Some(path.clone()));
                } else {
                    visit(child, path, sites);
                }
                path.pop();
            }
        }
    }
    let mut sites = BTreeMap::new();
    visit(graph, &mut Vec::new(), &mut sites);
    sites
        .into_iter()
        .filter_map(|(name, path)| Some((name, path?)))
        .collect()
}

#[derive(Debug, Snafu)]
pub enum CallbackError {
    #[snafu(display("host callback protocol: {reason}"), visibility(pub))]
    Protocol { reason: &'static str },
    #[snafu(display("host callback failed: {source}"), visibility(pub))]
    Handler {
        source: Box<dyn Error + Send + Sync>,
    },
    #[snafu(display("host callback encoding failed: {source}"), visibility(pub))]
    Codec { source: serde_json::Error },
    #[snafu(display("host callback I/O failed: {source}"), visibility(pub))]
    Transport { source: std::io::Error },
    #[snafu(transparent)]
    Inputs { source: crate::WorkflowInputError },
    #[snafu(transparent)]
    Stream { source: crate::StreamError },
    #[snafu(display("invalid callback result port {port}: {source}"))]
    OutputType {
        port: String,
        source: crate::TypeMismatch,
    },
}

type Handler = dyn Fn(&WorkflowArguments, &StreamCancellation) -> Result<CallbackReply, CallbackError>
    + Send
    + Sync;

#[derive(Clone)]
pub struct HostCallback(Arc<Handler>);

impl fmt::Debug for HostCallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HostCallback")
    }
}

impl HostCallback {
    pub fn new(
        f: impl Fn(&WorkflowArguments, &StreamCancellation) -> Result<CallbackReply, CallbackError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(f))
    }
    pub fn call(
        &self,
        args: &WorkflowArguments,
        cancellation: &StreamCancellation,
    ) -> Result<CallbackReply, CallbackError> {
        if let Some(error) = cancellation.failure() {
            return Err(error.into());
        }
        (self.0)(args, cancellation)
    }
}

/// Abandoning a reply (including a partial wire write) does not count as delivery.
pub struct CallbackReply {
    pub outputs: FlowOutputs,
    delivery: Option<Box<dyn FnOnce(bool) + Send>>,
}
impl From<FlowOutputs> for CallbackReply {
    fn from(outputs: FlowOutputs) -> Self {
        Self {
            outputs,
            delivery: None,
        }
    }
}
impl CallbackReply {
    pub fn on_delivery(mut self, f: impl FnOnce(bool) + Send + 'static) -> Self {
        self.delivery = Some(Box::new(f));
        self
    }
    pub fn delivered(mut self) -> FlowOutputs {
        if let Some(f) = self.delivery.take() {
            f(true);
        }
        std::mem::take(&mut self.outputs)
    }
    pub fn validate(&self, ports: &[PortSpec]) -> Result<(), CallbackError> {
        ensure!(
            self.outputs
                .keys()
                .all(|k| ports.iter().any(|p| p.name == k.as_str())),
            ProtocolSnafu {
                reason: "undeclared callback output"
            }
        );
        for port in ports {
            if let Some(value) = self.outputs.get(port.name.as_ref()) {
                port.value_type
                    .validate_shared(value)
                    .with_context(|_| OutputTypeSnafu {
                        port: port.name.to_string(),
                    })?;
            } else {
                ensure!(
                    !port.required,
                    ProtocolSnafu {
                        reason: "missing callback output"
                    }
                );
            }
        }
        Ok(())
    }
}
impl Drop for CallbackReply {
    fn drop(&mut self) {
        if let Some(f) = self.delivery.take() {
            f(false);
        }
    }
}

/// Business JSON does not carry native sensitivity markers. Preserve private subtrees
/// without tainting unrelated siblings when the caller appends a returned list.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireValues {
    values: NodeValues,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    private: Vec<Vec<String>>,
}
impl WireValues {
    pub fn encode(values: &NodeValues) -> Self {
        fn visit(path: &mut Vec<String>, value: &crate::ValueRef, private: &mut Vec<Vec<String>>) {
            if value.is_secret() {
                private.push(path.clone());
            } else if let Some(items) = value.as_array() {
                for (i, item) in items.iter().enumerate() {
                    path.push(i.to_string());
                    visit(path, item, private);
                    path.pop();
                }
            } else if let Some(items) = value.as_object() {
                for (k, item) in items {
                    path.push(k.to_string());
                    visit(path, item, private);
                    path.pop();
                }
            }
        }
        let mut private = Vec::new();
        for (name, value) in values {
            visit(&mut vec![name.clone()], value, &mut private);
        }
        Self {
            values: values.clone(),
            private,
        }
    }
    pub fn decode(self) -> Result<NodeValues, CallbackError> {
        use std::collections::BTreeSet;
        fn restore(
            value: &crate::ValueRef,
            path: &mut Vec<String>,
            private: &mut BTreeSet<Vec<String>>,
        ) -> crate::ValueRef {
            if private.is_empty() {
                return value.clone();
            }
            if private.remove(path) {
                return value.clone().into_secret();
            }
            if let Some(items) = value.as_array() {
                return crate::ValueRef::array(items.iter().enumerate().map(|(i, v)| {
                    path.push(i.to_string());
                    let value = restore(v, path, private);
                    path.pop();
                    value
                }));
            }
            if let Some(items) = value.as_object() {
                return crate::ValueRef::object(items.iter().map(|(k, v)| {
                    path.push(k.to_string());
                    let value = restore(v, path, private);
                    path.pop();
                    (k.clone(), value)
                }));
            }
            value.clone()
        }
        let mut private = BTreeSet::from_iter(self.private);
        let result = self
            .values
            .into_iter()
            .map(|(name, value)| {
                let v = restore(&value, &mut vec![name.clone()], &mut private);
                (name, v)
            })
            .collect();
        ensure!(
            private.is_empty(),
            ProtocolSnafu {
                reason: "invalid sensitivity path"
            }
        );
        Ok(result)
    }
}

pub const MAX_CALLBACK_FRAME: usize = 2 * 1024 * 1024;
