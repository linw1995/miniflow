use super::*;
use serde::{Deserializer, Serializer, de, ser};
use serde_json::{Map, json};
use snafu::{ResultExt, Snafu, ensure};
use std::collections::BTreeSet;

#[derive(Debug, Snafu)]
enum WireError {
    #[snafu(display("invalid workflow scope `{path}`: {message}"))]
    Invalid { path: String, message: &'static str },
    #[snafu(display("invalid workflow scope `{path}`: {source}"))]
    Json {
        path: String,
        source: serde_json::Error,
    },
}

impl<'de> Deserialize<'de> for WorkflowDefinition {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = crate::workflow_inputs::UniqueValue::deserialize(deserializer)?.0;
        let version: WorkflowDefinitionVersion =
            serde_json::from_value(value.get("version").cloned().unwrap_or(Value::Null))
                .map_err(de::Error::custom)?;
        let modern = version.supports_node_success_edges();
        decode_scope(&mut value, modern, version, "workflow").map_err(de::Error::custom)?;
        DefinitionFields::deserialize(value).map_err(de::Error::custom)
    }
}

impl Serialize for WorkflowDefinition {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        struct Canonical<'a>(&'a WorkflowDefinition);
        impl Serialize for Canonical<'_> {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                DefinitionFields::serialize(self.0, serializer)
            }
        }
        let mut value = serde_json::to_value(Canonical(self)).map_err(ser::Error::custom)?;
        encode_scope(
            &mut value,
            self.version.supports_node_success_edges(),
            self.version,
            "workflow",
        )
        .map_err(ser::Error::custom)?;
        value.serialize(serializer)
    }
}

fn decode_scope(
    value: &mut Value,
    modern: bool,
    version: WorkflowDefinitionVersion,
    path: &str,
) -> Result<(), WireError> {
    let Some(scope) = value.as_object_mut() else {
        return Ok(());
    };
    ensure!(
        version.supports_child_streams()
            || scope
                .get("nodes")
                .and_then(Value::as_array)
                .is_none_or(|nodes| nodes.iter().all(|node| {
                    node.pointer("/loop/stream_outputs")
                        .is_none_or(|outputs| outputs.as_object().is_some_and(Map::is_empty))
                        && (node["kind"] != crate::ITERATION_KIND
                            || node
                                .pointer("/config/stream_outputs")
                                .is_none_or(|outputs| {
                                    outputs.as_object().is_some_and(Map::is_empty)
                                }))
                })),
        InvalidSnafu {
            path,
            message: "Child streams require version 2026-10-28"
        }
    );
    ensure!(
        version.supports_nested_flows()
            || (!scope.contains_key("flows")
                && scope
                    .get("nodes")
                    .and_then(Value::as_array)
                    .is_none_or(|nodes| nodes.iter().all(|node| node["kind"] != crate::CALL_KIND))),
        InvalidSnafu {
            path,
            message: "nested flows require version 2026-10-25"
        }
    );
    ensure!(
        !scope.contains_key("order_edges"),
        InvalidSnafu {
            path,
            message: "order_edges is an internal graph field"
        }
    );
    ensure!(
        !scope.contains_key("calls") || version.supports_directed_calls(),
        InvalidSnafu {
            path,
            message: "directed calls require version 2026-10-28"
        }
    );
    ensure!(
        scope
            .get("clusters")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
            || version.supports_clusters(),
        InvalidSnafu {
            path,
            message: "node clusters require version 2026-10-23"
        }
    );
    if modern {
        ensure!(
            !scope.contains_key("control_edges"),
            InvalidSnafu {
                path,
                message: "control_edges is not supported in version 2026-10-19; use edges"
            }
        );
        let edges = scope.remove("edges").unwrap_or_else(|| json!([]));
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ExecutionEdge {
            from_node: DefinitionId,
            to_node: DefinitionId,
            #[serde(default, deserialize_with = "output")]
            from_output: Option<String>,
        }
        fn output<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
            let port = String::deserialize(d)?;
            if port.is_empty() {
                return Err(de::Error::custom("source output must not be empty"));
            }
            Ok(Some(port))
        }
        let edges: Vec<ExecutionEdge> =
            serde_json::from_value(edges).context(JsonSnafu { path })?;
        let mut controls = Vec::new();
        let mut orders = Vec::new();
        for edge in edges {
            if let Some(port) = edge.from_output {
                controls.push(
                    json!({"from_node":edge.from_node,"from_output":port,"to_node":edge.to_node}),
                );
            } else {
                orders.push(json!({"from_node":edge.from_node,"to_node":edge.to_node}));
            }
        }
        scope.insert("control_edges".into(), controls.into());
        if !orders.is_empty() {
            scope.insert("order_edges".into(), orders.into());
        }
        let mut data = Vec::new();
        if let Some(nodes) = scope.get_mut("nodes").and_then(Value::as_array_mut) {
            for node in nodes {
                let target = node.get("id").cloned().unwrap_or(Value::Null);
                let Some(node) = node.as_object_mut() else {
                    continue;
                };
                if let Some(bindings) = node.remove("inputs") {
                    let input_path = crate::workflow_inputs::pointer(
                        &crate::workflow_inputs::pointer(path, target.as_str().unwrap_or("?")),
                        "inputs",
                    );
                    let bindings: BTreeMap<String, Value> = serde_json::from_value(bindings)
                        .context(JsonSnafu { path: &input_path })?;
                    for (input, value) in bindings {
                        let binding_path = crate::workflow_inputs::pointer(&input_path, &input);
                        let path = binding_path.as_str();
                        let binding: InputBindingDefinition =
                            serde_json::from_value(value).context(JsonSnafu { path })?;
                        let (exclusive, sources) = match binding {
                            InputBindingDefinition::From { from } => (false, vec![from]),
                            InputBindingDefinition::Exclusive { exclusive } => {
                                ensure!(
                                    exclusive.len() >= 2
                                        && exclusive.iter().collect::<BTreeSet<_>>().len()
                                            == exclusive.len(),
                                    InvalidSnafu {
                                        path,
                                        message: "exclusive inputs require at least two distinct sources"
                                    }
                                );
                                (true, exclusive)
                            }
                        };
                        for source in sources {
                            ensure!(
                                !source.port.is_empty(),
                                InvalidSnafu {
                                    path,
                                    message: "source port must not be empty"
                                }
                            );
                            let mut edge = json!({"from_node":source.node,"from_output":source.port,"to_node":target,"to_input":input});
                            if exclusive {
                                edge["exclusive"] = json!(true);
                            }
                            data.push(edge);
                        }
                    }
                }
            }
        }
        fn edge_key(edge: &Value) -> [Option<&str>; 4] {
            ["from_node", "from_output", "to_node", "to_input"].map(|field| edge[field].as_str())
        }
        data.sort_by(|left, right| edge_key(left).cmp(&edge_key(right)));
        scope.insert("edges".into(), data.into());
    } else if let Some(nodes) = scope.get("nodes").and_then(Value::as_array) {
        ensure!(
            nodes.iter().all(|node| node.get("inputs").is_none()),
            InvalidSnafu {
                path,
                message: "node-local inputs require version 2026-10-19"
            }
        );
    }
    visit_bodies(scope, modern, version, path, decode_scope)
}

fn encode_scope(
    value: &mut Value,
    modern: bool,
    version: WorkflowDefinitionVersion,
    path: &str,
) -> Result<(), WireError> {
    let Some(scope) = value.as_object_mut() else {
        return Ok(());
    };
    ensure!(
        version.supports_child_streams()
            || scope
                .get("nodes")
                .and_then(Value::as_array)
                .is_none_or(|nodes| nodes.iter().all(|node| {
                    node.pointer("/loop/stream_outputs")
                        .is_none_or(|outputs| outputs.as_object().is_some_and(Map::is_empty))
                        && (node["kind"] != crate::ITERATION_KIND
                            || node
                                .pointer("/config/stream_outputs")
                                .is_none_or(|outputs| {
                                    outputs.as_object().is_some_and(Map::is_empty)
                                }))
                })),
        InvalidSnafu {
            path,
            message: "Child streams require version 2026-10-28"
        }
    );
    ensure!(
        version.supports_nested_flows()
            || (!scope.contains_key("flows")
                && scope
                    .get("nodes")
                    .and_then(Value::as_array)
                    .is_none_or(|nodes| nodes.iter().all(|node| node["kind"] != crate::CALL_KIND))),
        InvalidSnafu {
            path,
            message: "nested flows require version 2026-10-25"
        }
    );
    ensure!(
        !scope.contains_key("calls") || version.supports_directed_calls(),
        InvalidSnafu {
            path,
            message: "directed calls require version 2026-10-28"
        }
    );
    ensure!(
        scope
            .get("clusters")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
            || version.supports_clusters(),
        InvalidSnafu {
            path,
            message: "node clusters require version 2026-10-23"
        }
    );
    if modern {
        let data: Vec<EdgeDefinition> =
            serde_json::from_value(scope.remove("edges").unwrap_or_else(|| json!([])))
                .context(JsonSnafu { path })?;
        let mut groups: BTreeMap<(DefinitionId, String), Vec<EdgeDefinition>> = BTreeMap::new();
        for edge in data {
            groups
                .entry((edge.to_node.clone(), edge.to_input.clone()))
                .or_default()
                .push(edge);
        }
        if let Some(nodes) = scope.get_mut("nodes").and_then(Value::as_array_mut) {
            for node in nodes {
                let Some(id) = node.get("id").and_then(Value::as_str).map(str::to_owned) else {
                    continue;
                };
                let mut inputs = Map::new();
                let keys: Vec<_> = groups
                    .keys()
                    .filter(|(target, _)| target.as_str() == id)
                    .cloned()
                    .collect();
                for key in keys {
                    let edges = groups.remove(&key).expect("collected existing group");
                    let sources: Vec<_> = edges
                        .iter()
                        .map(|edge| json!({"node":edge.from_node,"port":edge.from_output}))
                        .collect();
                    let binding = if edges.iter().all(|edge| edge.exclusive) {
                        json!({"exclusive":sources})
                    } else {
                        ensure!(
                            edges.len() == 1 && !edges[0].exclusive,
                            InvalidSnafu {
                                path,
                                message: "input has conflicting bindings"
                            }
                        );
                        json!({"from":sources[0]})
                    };
                    inputs.insert(key.1, binding);
                }
                if !inputs.is_empty() {
                    node.as_object_mut()
                        .expect("node with ID")
                        .insert("inputs".into(), inputs.into());
                }
            }
        }
        ensure!(
            groups.is_empty(),
            InvalidSnafu {
                path,
                message: "data binding targets an unknown node"
            }
        );
        let mut edges = scope
            .remove("control_edges")
            .unwrap_or_else(|| json!([]))
            .as_array()
            .cloned()
            .unwrap_or_default();
        if let Some(orders) = scope.remove("order_edges") {
            edges.extend(orders.as_array().cloned().unwrap_or_default());
        }
        scope.insert("edges".into(), edges.into());
    } else {
        ensure!(
            scope
                .get("order_edges")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty),
            InvalidSnafu {
                path,
                message: "node-success edges require version 2026-10-19"
            }
        );
        scope.remove("order_edges");
    }
    visit_bodies(scope, modern, version, path, encode_scope)
}

fn visit_bodies(
    scope: &mut Map<String, Value>,
    modern: bool,
    version: WorkflowDefinitionVersion,
    path: &str,
    visit: fn(&mut Value, bool, WorkflowDefinitionVersion, &str) -> Result<(), WireError>,
) -> Result<(), WireError> {
    if let Some(nodes) = scope.get_mut("nodes").and_then(Value::as_array_mut) {
        for node in nodes {
            let nested = format!(
                "{path}/{}",
                node.get("id").and_then(Value::as_str).unwrap_or("?")
            );
            if let Some(body) = node.get_mut("loop").and_then(|value| value.get_mut("body")) {
                visit(body, modern, version, &nested)?;
            }
            if node.get("kind").and_then(Value::as_str) == Some(crate::ITERATION_KIND)
                && let Some(body) = node
                    .get_mut("config")
                    .and_then(|value| value.get_mut("body"))
            {
                visit(body, modern, version, &nested)?;
            }
        }
    }
    Ok(())
}
