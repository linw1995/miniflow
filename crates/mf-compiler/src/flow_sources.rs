use crate::{NodeDependency, WorkflowDefinition};
use mf_runtime::{CALL_KIND, CallConfig, FlowSource, MAX_CALL_DEPTH, NodeDefinition};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Snafu)]
pub enum FlowResolutionError {
    #[snafu(transparent)]
    BuiltinRegistry {
        source: mf_runtime::BuiltinFlowRegistryError,
    },
    #[snafu(display("could not resolve Flow source {path:?}: {source}"))]
    Source {
        path: PathBuf,
        #[snafu(source(from(FlowResolutionError, Box::new)))]
        source: Box<FlowResolutionError>,
    },
    #[snafu(display("could not read Flow source {path:?}: {source}"))]
    Read { path: PathBuf, source: io::Error },
    #[snafu(display("could not parse Flow source {path:?}: {source}"))]
    Parse {
        path: PathBuf,
        source: crate::DefinitionParseError,
    },
    #[snafu(display("invalid Flow call `{node}`: {source}"))]
    Configuration {
        node: String,
        source: serde_json::Error,
    },
    #[snafu(display("could not expand built-in Flow `{reference}`: {source}"))]
    Expansion {
        reference: String,
        source: mf_runtime::NodeBuildError,
    },
    #[snafu(display("invalid Flow reference `{reference}`: {message}"))]
    Invalid { reference: String, message: String },
    #[snafu(display("conflicting provider sources for package `{package}` in Flow `{reference}`"))]
    Dependency { package: String, reference: String },
}

pub struct ResolvedWorkflow {
    pub definition: WorkflowDefinition,
    pub sources: BTreeSet<PathBuf>,
}

#[derive(Default)]
struct Resolver<'a> {
    registry: Option<&'a mf_runtime::NodeRegistry>,
    host_dependencies: bool,
    stack: Vec<PathBuf>,
    sources: BTreeSet<PathBuf>,
    dependencies: BTreeMap<String, NodeDependency>,
}

#[cfg(feature = "codegen")]
pub fn resolve_embedded_flows(
    definition: &WorkflowDefinition,
    directory: &Path,
) -> Result<ResolvedWorkflow, FlowResolutionError> {
    let mut resolver = Resolver {
        host_dependencies: true,
        ..Default::default()
    };
    let mut definition = definition.clone();
    resolver.definition(
        &mut definition,
        Some(directory),
        &BTreeMap::new(),
        0,
        "workflow",
    )?;
    Ok(ResolvedWorkflow {
        definition,
        sources: resolver.sources,
    })
}

pub fn load_workflow(path: &Path) -> Result<ResolvedWorkflow, FlowResolutionError> {
    let mut resolver = Resolver::default();
    let mut definition = resolver.file(path, &BTreeMap::new(), 0)?;
    resolver.finish(&mut definition);
    Ok(ResolvedWorkflow {
        definition,
        sources: resolver.sources,
    })
}

/// Loads Flow files while the embedding host retains authority over provider dependencies.
pub fn load_embedded_workflow(path: &Path) -> Result<ResolvedWorkflow, FlowResolutionError> {
    let mut resolver = Resolver {
        host_dependencies: true,
        ..Default::default()
    };
    let definition = resolver.file(path, &BTreeMap::new(), 0)?;
    Ok(ResolvedWorkflow {
        definition,
        sources: resolver.sources,
    })
}

pub fn resolve_flows(
    definition: &WorkflowDefinition,
    directory: Option<&Path>,
) -> Result<ResolvedWorkflow, FlowResolutionError> {
    resolve_flows_with_registry(definition, directory, None)
}

pub fn resolve_flows_with_registry(
    definition: &WorkflowDefinition,
    directory: Option<&Path>,
    registry: Option<&mf_runtime::NodeRegistry>,
) -> Result<ResolvedWorkflow, FlowResolutionError> {
    if directory.is_none() && !needs_resolution(definition, registry) {
        return Ok(ResolvedWorkflow {
            definition: definition.clone(),
            sources: BTreeSet::new(),
        });
    }
    let mut resolver = Resolver {
        registry,
        ..Default::default()
    };
    let mut definition = definition.clone();
    resolver.definition(&mut definition, directory, &BTreeMap::new(), 0, "workflow")?;
    resolver.finish(&mut definition);
    Ok(ResolvedWorkflow {
        definition,
        sources: resolver.sources,
    })
}

fn needs_resolution(
    definition: &WorkflowDefinition,
    registry: Option<&mf_runtime::NodeRegistry>,
) -> bool {
    !definition.flows.is_empty()
        || definition
            .nodes
            .iter()
            .any(|node| node_needs_resolution(node, registry))
}

fn wire_node_needs_resolution(
    node: &serde_json::Value,
    registry: Option<&mf_runtime::NodeRegistry>,
) -> bool {
    matches!(node["kind"].as_str(), Some(CALL_KIND))
        || node["kind"]
            .as_str()
            .is_some_and(|kind| crate::calls::is_metadata(kind, registry))
        || [
            node.pointer("/loop/body/nodes"),
            node.pointer("/config/body/nodes"),
        ]
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_array)
        .flatten()
        .any(|node| wire_node_needs_resolution(node, registry))
}

fn node_needs_resolution(
    node: &NodeDefinition,
    registry: Option<&mf_runtime::NodeRegistry>,
) -> bool {
    crate::calls::is_flow_with_registry(node, registry)
        || node.loop_definition.as_ref().is_some_and(|body| {
            body.body
                .nodes
                .iter()
                .any(|node| node_needs_resolution(node, registry))
        })
        || (node.kind == mf_runtime::ITERATION_KIND
            && node
                .config
                .pointer("/body/nodes")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|nodes| {
                    nodes
                        .iter()
                        .any(|node| wire_node_needs_resolution(node, registry))
                }))
}

impl Resolver<'_> {
    fn finish(&self, definition: &mut WorkflowDefinition) {
        for dependency in self.dependencies.values() {
            if let Some(existing) = definition
                .dependencies
                .values_mut()
                .find(|value| value.package == dependency.package)
            {
                *existing = dependency.clone();
            } else {
                let mut index = definition.dependencies.len();
                let alias = loop {
                    let alias = format!("flow_dependency_{index}");
                    if !definition.dependencies.contains_key(&alias) {
                        break alias;
                    }
                    index += 1;
                };
                definition.dependencies.insert(alias, dependency.clone());
            }
        }
    }

    fn file(
        &mut self,
        path: &Path,
        inherited: &BTreeMap<String, NodeDependency>,
        depth: usize,
    ) -> Result<WorkflowDefinition, FlowResolutionError> {
        let path = fs::canonicalize(path).context(ReadSnafu { path })?;
        ensure!(
            !self.stack.contains(&path),
            InvalidSnafu {
                reference: path.display().to_string(),
                message: format!("recursive Flow reference through {:?}", self.stack)
            }
        );
        let source = fs::read_to_string(&path).context(ReadSnafu { path: &path })?;
        let mut definition =
            WorkflowDefinition::from_json(&source).context(ParseSnafu { path: &path })?;
        self.sources.insert(path.clone());
        self.stack.push(path.clone());
        let result = self.definition(
            &mut definition,
            path.parent(),
            inherited,
            depth,
            &path.display().to_string(),
        );
        self.stack.pop();
        result.context(SourceSnafu { path: &path })?;
        Ok(definition)
    }

    fn definition(
        &mut self,
        definition: &mut WorkflowDefinition,
        directory: Option<&Path>,
        inherited: &BTreeMap<String, NodeDependency>,
        depth: usize,
        reference: &str,
    ) -> Result<(), FlowResolutionError> {
        let resolve = needs_resolution(definition, self.registry);
        ensure!(
            depth <= MAX_CALL_DEPTH,
            InvalidSnafu {
                reference,
                message: "Flow nesting exceeds sixteen calls"
            }
        );
        ensure!(
            definition.flows.is_empty() || definition.version.supports_nested_flows(),
            InvalidSnafu {
                reference,
                message: "Flow sources require version 2026-10-25"
            }
        );
        for dependency in definition.dependencies.values_mut() {
            if !self.host_dependencies
                && let Some(path) = &mut dependency.path
                && let Some(directory) = directory
            {
                let absolute = directory.join(&*path);
                *path = fs::canonicalize(&absolute).context(ReadSnafu { path: absolute })?;
            }
            if !self.host_dependencies && (resolve || depth > 0) {
                self.merge(dependency, reference)?;
            }
        }
        if !resolve {
            return Ok(());
        }
        let mut available = inherited.clone();
        available.extend(
            definition
                .dependencies
                .values()
                .map(|value| (value.package.clone(), value.clone())),
        );
        let flows = std::mem::take(&mut definition.flows);
        for alias in flows.keys() {
            ensure!(
                !alias.trim().is_empty(),
                InvalidSnafu {
                    reference,
                    message: "Flow alias must not be blank"
                }
            );
        }
        self.nodes(
            &mut definition.nodes,
            &flows,
            directory,
            &available,
            depth,
            definition.version,
        )?;
        Ok(())
    }

    fn merge(
        &mut self,
        dependency: &NodeDependency,
        reference: &str,
    ) -> Result<(), FlowResolutionError> {
        if let Some(existing) = self.dependencies.get_mut(&dependency.package) {
            ensure!(
                existing.path == dependency.path
                    && existing.version == dependency.version
                    && existing.git == dependency.git
                    && existing.rev == dependency.rev,
                DependencySnafu {
                    package: &dependency.package,
                    reference
                }
            );
            existing
                .features
                .extend(dependency.features.iter().cloned());
            existing.features.sort();
            existing.features.dedup();
            existing.default_features |= dependency.default_features;
        } else {
            self.dependencies
                .insert(dependency.package.clone(), dependency.clone());
        }
        Ok(())
    }

    fn source(
        &mut self,
        source: &FlowSource,
        directory: Option<&Path>,
        available: &BTreeMap<String, NodeDependency>,
        depth: usize,
    ) -> Result<WorkflowDefinition, FlowResolutionError> {
        ensure!(
            depth <= MAX_CALL_DEPTH,
            InvalidSnafu {
                reference: "builtin",
                message: "Flow nesting exceeds sixteen calls"
            }
        );
        match source {
            FlowSource::Path { path } => {
                let directory = directory.context(InvalidSnafu {
                    reference: path.display().to_string(),
                    message: "local Flow references require load_workflow or a source directory",
                })?;
                self.file(&directory.join(path), available, depth)
            }
            FlowSource::Builtin {
                builtin,
                config,
                flows,
            } => {
                let registry = mf_runtime::BuiltinFlowRegistry::from_inventory()?;
                let registration = registry.get(builtin).context(InvalidSnafu {
                    reference: builtin,
                    message: "unknown built-in Flow; link its provider before resolving sources",
                })?;
                // Resolve injections in the caller's directory BEFORE entering the provider.
                let mut injected = BTreeMap::new();
                for (alias, source) in flows {
                    injected.insert(
                        alias.clone(),
                        self.source(source, directory, available, depth + 1)?,
                    );
                }
                let mut definition = if let Some(configure) = registration.configure {
                    configure(config.clone(), injected)
                        .context(ExpansionSnafu { reference: builtin })?
                } else {
                    ensure!(
                        config.is_null() && injected.is_empty(),
                        InvalidSnafu {
                            reference: builtin,
                            message: "this built-in Flow does not accept configuration or injected Flows"
                        }
                    );
                    (registration.definition)().context(ParseSnafu {
                        path: PathBuf::from(format!("builtin:{builtin}")),
                    })?
                };
                for dependency in definition.dependencies.values_mut() {
                    if let Some(selected) = available
                        .values()
                        .find(|selected| selected.package == dependency.package)
                    {
                        let features = std::mem::take(&mut dependency.features);
                        let default_features = dependency.default_features;
                        *dependency = selected.clone();
                        dependency.features.extend(features);
                        dependency.features.sort();
                        dependency.features.dedup();
                        dependency.default_features |= default_features;
                    }
                }
                self.definition(&mut definition, None, available, depth, builtin)?;
                Ok(definition)
            }
        }
    }

    fn nodes(
        &mut self,
        nodes: &mut [NodeDefinition],
        flows: &BTreeMap<String, FlowSource>,
        directory: Option<&Path>,
        available: &BTreeMap<String, NodeDependency>,
        depth: usize,
        version: mf_runtime::WorkflowDefinitionVersion,
    ) -> Result<(), FlowResolutionError> {
        for node in nodes {
            if crate::calls::is_flow_with_registry(node, self.registry) {
                let mut config: CallConfig = serde_json::from_value(
                    crate::calls::config_mut(node).clone(),
                )
                .context(ConfigurationSnafu {
                    node: node.id.as_str(),
                })?;
                ensure!(
                    version.supports_nested_flows(),
                    InvalidSnafu {
                        reference: node.id.as_str(),
                        message: "Flow calls require version 2026-10-25"
                    }
                );
                ensure!(
                    !config.flow.trim().is_empty(),
                    InvalidSnafu {
                        reference: node.id.as_str(),
                        message: "Flow alias must not be blank"
                    }
                );
                let mut child = if let Some(mut definition) = config.definition.take() {
                    self.definition(
                        &mut definition,
                        directory,
                        available,
                        depth + 1,
                        &config.flow,
                    )?;
                    definition
                } else {
                    let source = flows.get(&config.flow).ok_or_else(|| {
                        InvalidSnafu {
                            reference: &config.flow,
                            message: "unknown Flow alias",
                        }
                        .build()
                    })?;
                    Box::new(self.source(source, directory, available, depth + 1)?)
                };
                ensure!(
                    child.execution.is_none() || version.supports_child_streams(),
                    InvalidSnafu {
                        reference: &config.flow,
                        message: "Flow calls require a terminal child interface"
                    }
                );
                // The resolved body is frozen in the plan, so runtime never follows a source path.
                child.flows.clear();
                config.definition = Some(child);
                *crate::calls::config_mut(node) =
                    serde_json::to_value(config).context(ConfigurationSnafu {
                        node: node.id.as_str(),
                    })?;
            }
            if let Some(definition) = &mut node.loop_definition {
                self.nodes(
                    &mut definition.body.nodes,
                    flows,
                    directory,
                    available,
                    depth,
                    version,
                )?;
            }
            if node.kind == mf_runtime::ITERATION_KIND {
                let mut config: mf_runtime::IterationConfig =
                    serde_json::from_value(node.config.clone()).context(ConfigurationSnafu {
                        node: node.id.as_str(),
                    })?;
                self.nodes(
                    &mut config.body.nodes,
                    flows,
                    directory,
                    available,
                    depth,
                    version,
                )?;
                node.config = serde_json::to_value(config).context(ConfigurationSnafu {
                    node: node.id.as_str(),
                })?;
            }
        }
        Ok(())
    }
}
