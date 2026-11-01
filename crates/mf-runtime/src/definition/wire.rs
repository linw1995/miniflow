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

impl<'de> Deserialize<'de> for FlowBodyDefinition {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = crate::workflow_inputs::UniqueValue::deserialize(deserializer)?.0;
        decode_scope(&mut value, true, WorkflowDefinitionVersion::CURRENT, "body")
            .map_err(de::Error::custom)?;
        FlowBodyFields::deserialize(value).map_err(de::Error::custom)
    }
}

impl Serialize for FlowBodyDefinition {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        struct Canonical<'a>(&'a FlowBodyDefinition);
        impl Serialize for Canonical<'_> {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                FlowBodyFields::serialize(self.0, serializer)
            }
        }
        let mut value = serde_json::to_value(Canonical(self)).map_err(ser::Error::custom)?;
        encode_scope(&mut value, true, WorkflowDefinitionVersion::CURRENT, "body")
            .map_err(ser::Error::custom)?;
        value.serialize(serializer)
    }
}

// Inline bodies perform their own wire conversion, but their availability (and
// injection metadata) is controlled by the enclosing Flow's schema version.
fn validate_flow_sources(
    scope: &Map<String, Value>,
    version: WorkflowDefinitionVersion,
    path: &str,
) -> Result<(), WireError> {
    fn source(
        value: &Value,
        version: WorkflowDefinitionVersion,
        path: &str,
    ) -> Result<(), WireError> {
        let arguments = value.get("flows").and_then(Value::as_object);
        ensure!(
            version.supports_flow_arguments()
                || (value.get("body").is_none() && arguments.is_none_or(Map::is_empty)),
            InvalidSnafu {
                path,
                message: "Flow arguments and inline bodies require version 2026-11-01"
            }
        );
        if let Some(arguments) = arguments {
            for (alias, argument) in arguments {
                if let Some(nested) = argument.get("source") {
                    source(nested, version, &format!("{path}/flows/{alias}/source"))?;
                }
            }
        }
        Ok(())
    }
    if let Some(flows) = scope.get("flows").and_then(Value::as_object) {
        for (alias, value) in flows {
            source(value, version, &format!("{path}/flows/{alias}"))?;
        }
    }
    Ok(())
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
    validate_flow_sources(scope, version, path)?;
    ensure!(
        !scope.contains_key("order_edges"),
        InvalidSnafu {
            path,
            message: "order_edges is an internal graph field"
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
    validate_flow_sources(scope, version, path)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> Value {
        json!({
            "nodes": [
                {"id": "source", "kind": "test.source"},
                {"id": "sink", "kind": "test.sink", "inputs": {
                    "value": {"from": {"node": "source", "port": "value"}}
                }}
            ],
            "edges": [{"from_node": "source", "to_node": "sink"}],
            "outputs": [{"name": "result", "node": "sink", "port": "result"}],
            "clusters": [{"label": "work", "annotation": "body", "nodes": ["source", "sink"]}]
        })
    }

    fn workflow(source: Value) -> Value {
        json!({
            "version": "2026-11-01", "dependencies": {}, "nodes": [],
            "flows": {"child": source}
        })
    }

    #[test]
    fn inline_body_roundtrips_canonical_bindings_and_execution_edges() {
        let input = workflow(json!({"builtin": "test.agent", "flows": {
            "tool": {"source": {"body": body()}, "description": "A tool", "parameters": {
                "source.value": {"description": "The input"}
            }}
        }}));
        let definition: WorkflowDefinition = serde_json::from_value(input).unwrap();
        let FlowSource::Builtin { flows, .. } = &definition.flows["child"] else {
            panic!("expected builtin");
        };
        let FlowSource::Body { body } = &flows["tool"].source else {
            panic!("expected inline body");
        };
        assert_eq!(body.edges.len(), 1);
        assert_eq!(body.order_edges.len(), 1);
        assert_eq!(body.clusters.len(), 1);
        let encoded = serde_json::to_value(&definition).unwrap();
        let encoded_body = &encoded["flows"]["child"]["flows"]["tool"]["source"]["body"];
        assert_eq!(
            encoded_body["nodes"][1]["inputs"]["value"]["from"]["node"],
            "source"
        );
        assert_eq!(
            encoded_body["edges"],
            json!([{"from_node":"source", "to_node":"sink"}])
        );
        assert!(encoded_body.get("order_edges").is_none());
        assert_eq!(
            serde_json::from_value::<WorkflowDefinition>(encoded).unwrap(),
            definition
        );
    }

    #[test]
    fn inline_body_preserves_nested_loop_and_iteration_bindings() {
        let mut inline = body();
        let nested_edges = json!([
            {"from_node": "source", "to_node": "sink"},
            {"from_node": "source", "from_output": "value", "to_node": "sink"}
        ]);
        let mut loop_body = body();
        loop_body["edges"] = nested_edges.clone();
        loop_body.as_object_mut().unwrap().remove("outputs");
        inline["nodes"].as_array_mut().unwrap().extend([
            json!({
                "id": "repeat", "kind": "workflow.loop", "loop": {
                    "max_iterations": 1, "variables": [], "body": loop_body
                }
            }),
            json!({
                "id": "each", "kind": "builtin.iteration", "config": {
                    "body": {
                        "nodes": body()["nodes"], "edges": nested_edges,
                        "result": {"node": "sink", "port": "result"}
                    }
                }
            }),
        ]);
        let definition: WorkflowDefinition =
            serde_json::from_value(workflow(json!({"body": inline}))).unwrap();
        let encoded = serde_json::to_value(&definition).unwrap();
        let nodes = &encoded["flows"]["child"]["body"]["nodes"];
        for nested in [&nodes[2]["loop"]["body"], &nodes[3]["config"]["body"]] {
            assert_eq!(
                nested["nodes"][1]["inputs"]["value"]["from"]["node"],
                "source"
            );
            let edges = nested["edges"].as_array().unwrap();
            assert_eq!(edges.len(), 2);
            for edge in nested_edges.as_array().unwrap() {
                assert!(edges.contains(edge));
            }
            assert!(nested.get("order_edges").is_none());
            assert!(nested.get("control_edges").is_none());
        }
        assert_eq!(
            serde_json::from_value::<WorkflowDefinition>(encoded).unwrap(),
            definition
        );
    }

    #[test]
    fn flow_arguments_and_bodies_require_new_version_on_decode_and_encode() {
        for source in [
            json!({"body": body()}),
            json!({"builtin": "test.agent", "flows": {
                "tool": {"source": {"path": "tool.json"}, "description": "A tool"}
            }}),
        ] {
            let mut input = workflow(source);
            let mut definition: WorkflowDefinition = serde_json::from_value(input.clone()).unwrap();
            input["version"] = json!("2026-10-28");
            assert!(serde_json::from_value::<WorkflowDefinition>(input).is_err());
            definition.version = WorkflowDefinitionVersion::V2026_10_28;
            assert!(serde_json::to_value(definition).is_err());
        }
        let mut bare = workflow(json!({"builtin": "test.agent"}));
        bare["version"] = json!("2026-10-28");
        let definition: WorkflowDefinition = serde_json::from_value(bare).unwrap();
        assert!(serde_json::to_value(definition).is_ok());
    }

    #[test]
    fn injection_fields_are_strict_and_workflow_annotation_is_removed() {
        for argument in [
            json!({"source": {"path": "tool.json"}, "extra": true}),
            json!({"source": {"path": "tool.json"}, "parameters": {
                "input": {"description": "Input", "extra": true}
            }}),
            json!({"path": "tool.json"}),
        ] {
            assert!(serde_json::from_value::<FlowArgument>(argument).is_err());
        }
        let mut input = workflow(json!({"builtin": "test.agent"}));
        input["annotation"] = json!("not supported");
        assert!(serde_json::from_value::<WorkflowDefinition>(input).is_err());
        let mut invalid_body = body();
        invalid_body["dependencies"] = json!({});
        assert!(serde_json::from_value::<FlowBodyDefinition>(invalid_body).is_err());
    }
}
