use crate::{NodeBuildError, PreparedSubgraph};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use snafu::ensure;
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
    pub invocation: Option<FlowInvocationBinding>,
}

/// A runtime selection from an edge-carried collection of statically bounded Flows.
#[derive(Clone, Debug)]
pub struct FlowInvocationBinding {
    pub input: String,
    pub field: Option<String>,
}

/// A compiler-frozen route and the original capability that enables it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowCandidate {
    pub route: String,
    pub source: Vec<String>,
}

pub const FLOW_CANDIDATES: &str = "flow_candidates";

pub fn candidate_binding(index: usize) -> String {
    format!("flow_candidate_{index}")
}

/// Only an explicit frozen candidate table authorizes its corresponding bodies.
pub fn candidate_parameters(config: &Value) -> BTreeMap<String, Value> {
    let Some(candidates) = config.get(FLOW_CANDIDATES).and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    candidates
        .keys()
        .filter_map(|binding| {
            config
                .get(binding)
                .filter(|body| !body.is_null())
                .map(|body| (binding.clone(), body.clone()))
        })
        .collect()
}

/// Remove compiler-owned bodies before the provider's strict business decoder runs.
pub fn take_candidates(
    config: &mut Value,
) -> Result<Option<BTreeMap<String, FlowCandidate>>, NodeBuildError> {
    let Some(fields) = config.as_object_mut() else {
        return Ok(None);
    };
    let Some(candidates) = fields.remove(FLOW_CANDIDATES) else {
        return Ok(None);
    };
    let candidates: BTreeMap<String, FlowCandidate> = crate::deserialize_config(candidates)?;
    for (index, (binding, candidate)) in candidates.iter().enumerate() {
        ensure!(
            binding.starts_with("flow_candidate_")
                && binding["flow_candidate_".len()..].parse::<usize>().is_ok()
                && !candidate.source.is_empty()
                && candidate.source.len() <= mf_telemetry::MAX_SCOPE_DEPTH
                && candidate.source.iter().all(|key| !key.trim().is_empty())
                && fields.remove(binding).is_some_and(|body| !body.is_null()),
            crate::NodeInvalidSubgraphSnafu {
                message: format!("invalid frozen Flow candidate {index} `{binding}`"),
            }
        );
    }
    Ok(Some(candidates))
}

/// Matches a capability to its frozen route without treating metadata as an executor.
pub fn validate_candidate_source(
    selected: &crate::FlowValue,
    candidate: &FlowCandidate,
    body: &PreparedSubgraph,
) -> bool {
    selected
        .body()
        .origin()
        .zip(body.origin())
        .is_some_and(|(source, target)| {
            source.workflow_id == target.workflow_id && source.path == candidate.source
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_explicit_candidate_bodies_are_selected_and_removed() {
        for mut config in [Value::Null, json!([]), json!({"ordinary":{"nodes":[]}})] {
            assert!(candidate_parameters(&config).is_empty());
            assert!(take_candidates(&mut config).unwrap().is_none());
        }
        let binding = candidate_binding(0);
        let mut config = json!({
            "mode":"execute", "ordinary":{"nodes":[]},
            "flow_candidates":{&binding:{"route":"a/b~", "source":["producer"]}},
            &binding:{"nodes":[]}
        });
        assert_eq!(
            candidate_parameters(&config),
            BTreeMap::from([(binding.clone(), json!({"nodes":[]}))])
        );
        let candidates = take_candidates(&mut config).unwrap().unwrap();
        assert_eq!(candidates[&binding].route, "a/b~");
        assert_eq!(config, json!({"mode":"execute", "ordinary":{"nodes":[]}}));
        let mut empty = json!({"flow_candidates":{}});
        assert_eq!(take_candidates(&mut empty).unwrap(), Some(BTreeMap::new()));
    }

    #[test]
    fn malformed_or_missing_candidate_metadata_keeps_typed_errors() {
        for table in [
            json!([]),
            json!(1),
            Value::Null,
            json!({"flow_candidate_0":{"route":1,"source":[]}}),
        ] {
            let mut config = json!({"flow_candidates":table});
            assert!(matches!(
                take_candidates(&mut config),
                Err(NodeBuildError::InvalidConfiguration { .. })
            ));
        }
        for (binding, source, body) in [
            ("invalid", json!(["producer"]), json!({})),
            ("flow_candidate_x", json!(["producer"]), json!({})),
            ("flow_candidate_0", json!([]), json!({})),
            ("flow_candidate_0", json!([""]), json!({})),
            ("flow_candidate_0", json!(["producer"]), Value::Null),
            (
                "flow_candidate_0",
                json!(vec!["scope"; mf_telemetry::MAX_SCOPE_DEPTH + 1]),
                json!({}),
            ),
        ] {
            let mut config = json!({"flow_candidates":{binding:{"route":"tool", "source":source}}, binding:body});
            assert!(matches!(
                take_candidates(&mut config),
                Err(NodeBuildError::InvalidSubgraph { .. })
            ));
        }
        let mut missing =
            json!({"flow_candidates":{"flow_candidate_0":{"route":"tool", "source":["producer"]}}});
        assert!(candidate_parameters(&missing).is_empty());
        assert!(take_candidates(&mut missing).is_err());
    }

    #[test]
    fn matching_requires_both_the_published_workflow_and_original_definition() {
        let workflow_id =
            mf_telemetry::identity::WorkflowId::from_definition(&json!({"test":"candidates"}), &[])
                .unwrap();
        let body = || PreparedSubgraph::new(vec![], vec![], |_| Ok(Default::default()));
        let source = crate::FlowValue::new(body().with_origin(crate::FlowOrigin {
            workflow_id: workflow_id.clone(),
            path: vec!["producer".into()],
        }))
        .unwrap();
        let candidate = FlowCandidate {
            route: "tool".into(),
            source: vec!["producer".into()],
        };
        let target = body().with_origin(crate::FlowOrigin {
            workflow_id: workflow_id.clone(),
            path: vec!["execute".into()],
        });
        assert!(validate_candidate_source(&source, &candidate, &target));
        assert!(!validate_candidate_source(
            &source,
            &FlowCandidate {
                source: vec!["other".into()],
                ..candidate.clone()
            },
            &target
        ));
        assert!(!validate_candidate_source(&source, &candidate, &body()));
        let foreign = body().with_origin(crate::FlowOrigin {
            workflow_id: mf_telemetry::identity::WorkflowId::from_definition(
                &json!({"other":true}),
                &[],
            )
            .unwrap(),
            path: vec!["execute".into()],
        });
        assert!(!validate_candidate_source(&source, &candidate, &foreign));
        assert!(!validate_candidate_source(
            &crate::FlowValue::new(body()).unwrap(),
            &candidate,
            &target
        ));
    }
}
