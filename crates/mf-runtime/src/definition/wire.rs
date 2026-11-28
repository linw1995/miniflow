use super::*;
use serde::{Deserializer, Serializer, de, ser};
use serde_json::{Map, json};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
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
    #[snafu(transparent)]
    Exclusive { source: ExclusiveInputError },
    #[snafu(display("invalid input reference at `{path}`: {source}"))]
    Template {
        path: String,
        source: crate::ConfigTemplateError,
    },
    #[snafu(display("invalid input reference `{output}` at `{path}`: {reason}"))]
    InputReference {
        path: String,
        output: String,
        reason: &'static str,
    },
}

/// Structural planning runs before provider ports are known. Resolve against all node ID
/// prefixes in this scope, never by guessing the first or last dot in an output key.
struct ReferenceScope(BTreeSet<String>);

impl ReferenceScope {
    fn new(scope: &Map<String, Value>) -> Self {
        Self(
            scope
                .get("nodes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|node| node["id"].as_str())
                .chain([ROOT_SOURCE_ID, LOOP_SOURCE_ID, crate::ITERATION_INPUT_ID])
                .map(str::to_owned)
                .collect(),
        )
    }

    fn endpoint<'a>(&self, output: &'a str, path: &str) -> Result<(&'a str, &'a str), WireError> {
        let mut candidates = output.match_indices('.').filter_map(|(index, _)| {
            let (node, port) = (&output[..index], &output[index + 1..]);
            (!port.is_empty() && self.0.contains(node)).then_some((node, port))
        });
        let endpoint = candidates.next().context(InputReferenceSnafu {
            path,
            output,
            reason: "expected a source node in this scope followed by a nonempty port",
        })?;
        ensure!(
            candidates.next().is_none(),
            InputReferenceSnafu {
                path,
                output,
                reason: "ambiguous source node prefix; rename the overlapping node IDs"
            }
        );
        Ok(endpoint)
    }

    fn source(&self, value: Value, path: &str) -> Result<OutputReferenceDefinition, WireError> {
        let template = crate::ConfigTemplate::parse(value).context(TemplateSnafu { path })?;
        let crate::ConfigTemplate::Reference {
            output,
            path: pointer,
        } = template
        else {
            return InvalidSnafu {
                path,
                message: "expected a whole-output $ref",
            }
            .fail();
        };
        ensure!(
            pointer.is_empty(),
            InputReferenceSnafu {
                path,
                output: &output,
                reason: "input ports require a whole output, not a JSON Pointer projection"
            }
        );
        let (node, port) = self.endpoint(&output, path)?;
        Ok(OutputReferenceDefinition {
            node: node.into(),
            port: port.to_owned(),
        })
    }

    fn sources(
        &self,
        value: Value,
        path: &str,
    ) -> Result<(bool, Vec<OutputReferenceDefinition>), WireError> {
        if value.get("exclusive").is_none() {
            return Ok((false, vec![self.source(value, path)?]));
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Alternatives {
            exclusive: Vec<Value>,
        }
        let alternatives: Alternatives =
            serde_json::from_value(value).context(JsonSnafu { path })?;
        let exclusive = alternatives
            .exclusive
            .into_iter()
            .enumerate()
            .map(|(index, value)| self.source(value, &format!("{path}/exclusive/{index}")))
            .collect::<Result<Vec<_>, _>>()?;
        ensure!(
            exclusive.len() >= 2
                && exclusive.iter().collect::<BTreeSet<_>>().len() == exclusive.len(),
            InvalidSnafu {
                path,
                message: "exclusive inputs require at least two distinct sources"
            }
        );
        Ok((true, exclusive))
    }

    fn encode(&self, edge: &EdgeDefinition, path: &str) -> Result<Value, WireError> {
        let output = crate::output_id(edge.from_node.as_str(), &edge.from_output);
        let (node, _) = self.endpoint(&output, path)?;
        // Never serialize a programmatic edge into a reference to a different endpoint.
        ensure!(
            node == edge.from_node.as_str(),
            InputReferenceSnafu {
                path,
                output: &output,
                reason: "source node is not addressable in this scope"
            }
        );
        Ok(json!({"$ref": output}))
    }
}

impl<'de> Deserialize<'de> for WorkflowDefinition {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = crate::workflow_inputs::UniqueValue::deserialize(deserializer)?.0;
        let version: WorkflowDefinitionVersion =
            serde_json::from_value(value.get("version").cloned().unwrap_or(Value::Null))
                .map_err(de::Error::custom)?;
        decode_scope(&mut value, version, "workflow").map_err(de::Error::custom)?;
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
        encode_scope(&mut value, self.version, "workflow").map_err(ser::Error::custom)?;
        value.serialize(serializer)
    }
}

fn validate_scope_features(
    scope: &Map<String, Value>,
    version: WorkflowDefinitionVersion,
    path: &str,
) -> Result<(), WireError> {
    ensure!(
        version.supports_child_streams()
            || scope
                .get("nodes")
                .and_then(Value::as_array)
                .is_none_or(|nodes| nodes.iter().all(|node| {
                    !matches!(
                        node["kind"].as_str(),
                        Some(LOOP_KIND | crate::ITERATION_KIND)
                    ) || node
                        .pointer("/config/stream_outputs")
                        .is_none_or(|outputs| outputs.as_object().is_some_and(Map::is_empty))
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
                    .is_none_or(|nodes| {
                        nodes.iter().all(|node| {
                            node["kind"] != crate::CALL_KIND
                                || CallConfig::is_dynamic(&node["config"])
                        })
                    })),
        InvalidSnafu {
            path,
            message: "nested flows require version 2026-10-25"
        }
    );
    ensure!(
        version.supports_cluster_collapsed()
            || scope
                .get("clusters")
                .and_then(Value::as_array)
                .is_none_or(|clusters| {
                    clusters.iter().all(|cluster| {
                        cluster.get("collapsed").and_then(Value::as_bool) != Some(true)
                    })
                }),
        InvalidSnafu {
            path,
            message: "default-collapsed clusters require version 2026-11-11"
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
    Ok(())
}

fn decode_scope(
    value: &mut Value,
    version: WorkflowDefinitionVersion,
    path: &str,
) -> Result<(), WireError> {
    let Some(scope) = value.as_object_mut() else {
        return Ok(());
    };
    validate_scope_features(scope, version, path)?;
    ensure!(
        !scope.contains_key("order_edges"),
        InvalidSnafu {
            path,
            message: "order_edges is an internal graph field"
        }
    );
    ensure!(
        !scope.contains_key("control_edges"),
        InvalidSnafu {
            path,
            message: "use edges for execution prerequisites"
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
    let edges: Vec<ExecutionEdge> = serde_json::from_value(edges).context(JsonSnafu { path })?;
    let mut controls = Vec::new();
    let mut orders = Vec::new();
    for edge in edges {
        if let Some(port) = edge.from_output {
            controls.push(
                json!({"from_node":edge.from_node,"from_output":port,"to_node":edge.to_node}),
            );
        } else {
            ensure!(
                version.supports_node_success_edges(),
                InvalidSnafu {
                    path,
                    message: "node-success edges require version 2026-10-19"
                }
            );
            orders.push(json!({"from_node":edge.from_node,"to_node":edge.to_node}));
        }
    }
    scope.insert("control_edges".into(), controls.into());
    if !orders.is_empty() {
        scope.insert("order_edges".into(), orders.into());
    }
    let references = ReferenceScope::new(scope);
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
                let bindings: BTreeMap<String, Value> =
                    serde_json::from_value(bindings).context(JsonSnafu { path: &input_path })?;
                for (input, value) in bindings {
                    let binding_path = crate::workflow_inputs::pointer(&input_path, &input);
                    let path = binding_path.as_str();
                    let (exclusive, sources) = references.sources(value, path)?;
                    for source in sources {
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
    Ok(())
}

fn encode_scope(
    value: &mut Value,
    version: WorkflowDefinitionVersion,
    path: &str,
) -> Result<(), WireError> {
    let Some(scope) = value.as_object_mut() else {
        return Ok(());
    };
    validate_scope_features(scope, version, path)?;
    let data: Vec<EdgeDefinition> =
        serde_json::from_value(scope.remove("edges").unwrap_or_else(|| json!([])))
            .context(JsonSnafu { path })?;
    validate_exclusive_inputs(&data)?;
    let references = ReferenceScope::new(scope);
    let mut groups = BTreeMap::<String, BTreeMap<String, Vec<EdgeDefinition>>>::new();
    for edge in data {
        groups
            .entry(edge.to_node.to_string())
            .or_default()
            .entry(edge.to_input.clone())
            .or_default()
            .push(edge);
    }
    if let Some(nodes) = scope.get_mut("nodes").and_then(Value::as_array_mut) {
        for node in nodes {
            let Some(id) = node.get("id").and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            let mut inputs = Map::new();
            for (input, edges) in groups.remove(&id).into_iter().flatten() {
                let mut sources = edges
                    .iter()
                    .map(|edge| references.encode(edge, path))
                    .collect::<Result<Vec<_>, _>>()?;
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
                    sources.pop().expect("exactly one ordinary source")
                };
                inputs.insert(input, binding);
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
        ensure!(
            version.supports_node_success_edges(),
            InvalidSnafu {
                path,
                message: "node-success edges require version 2026-10-19"
            }
        );
        edges.extend(orders.as_array().cloned().unwrap_or_default());
    }
    scope.insert("edges".into(), edges.into());
    Ok(())
}
