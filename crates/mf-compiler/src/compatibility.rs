use serde_json::Value;
use snafu::Snafu;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Snafu)]
#[snafu(display("incompatible runtime dependencies: {details}"))]
pub struct RuntimeCompatibilityError {
    pub details: String,
}

pub fn validate_runtime_identity(metadata: &Value) -> Result<(), RuntimeCompatibilityError> {
    let packages = metadata["packages"]
        .as_array()
        .ok_or_else(|| RuntimeCompatibilityError {
            details: "Cargo metadata has no packages".into(),
        })?;
    let runtimes: Vec<_> = packages
        .iter()
        .filter(|p| p["name"] == "mf-runtime")
        .collect();
    let nodes =
        metadata["resolve"]["nodes"]
            .as_array()
            .ok_or_else(|| RuntimeCompatibilityError {
                details: "Cargo metadata has no dependency graph".into(),
            })?;
    let root = metadata["resolve"]["root"]
        .as_str()
        .ok_or_else(|| RuntimeCompatibilityError {
            details: "Cargo metadata has no root".into(),
        })?;
    let root_node = nodes.iter().find(|node| node["id"] == root);
    let required = root_node
        .and_then(|n| n["deps"].as_array())
        .and_then(|deps| deps.iter().find(|d| d["name"] == "mf_runtime"))
        .and_then(|d| d["pkg"].as_str());
    if runtimes.len() == 1 && required.is_some() && runtimes[0]["id"].as_str() == required {
        return Ok(());
    }
    let graph: BTreeMap<_, _> = nodes
        .iter()
        .filter_map(|n| Some((n["id"].as_str()?, n["deps"].as_array()?)))
        .collect();
    let mut queue = VecDeque::from([(root.to_owned(), vec![root.to_owned()])]);
    let mut paths = BTreeMap::new();
    let mut seen = BTreeSet::new();
    while let Some((id, path)) = queue.pop_front() {
        if !seen.insert(id.clone()) {
            continue;
        }
        paths.insert(id.clone(), path.clone());
        if let Some(deps) = graph.get(id.as_str()) {
            for dep in *deps {
                if let Some(child) = dep["pkg"].as_str() {
                    let mut next = path.clone();
                    next.push(child.to_owned());
                    queue.push_back((child.to_owned(), next));
                }
            }
        }
    }
    let details: Vec<_> = runtimes
        .iter()
        .map(|p| {
            let id = p["id"].as_str().unwrap_or("unknown runtime");
            paths
                .get(id)
                .map(|p| p.join(" -> "))
                .unwrap_or_else(|| id.to_owned())
        })
        .collect();
    Err(RuntimeCompatibilityError {
        details: format!(
            "expected one mf-runtime identity required by the runner; found {}: {}",
            runtimes.len(),
            details.join("; ")
        ),
    })
}
