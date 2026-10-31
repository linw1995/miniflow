use crate::{NodeDependency, WorkflowDefinition, subgraphs::ResolvedSubgraphSource};
use mf_runtime::{
    CALL_KIND, CallConfig, MAX_CALL_DEPTH, NodeDefinition, SubgraphSource, SubgraphSourceKind,
};
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
struct Resolver {
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
        &BTreeMap::new(),
    )?;
    Ok(ResolvedWorkflow {
        definition,
        sources: resolver.sources,
    })
}

pub fn load_workflow(path: &Path) -> Result<ResolvedWorkflow, FlowResolutionError> {
    let mut resolver = Resolver::default();
    let mut definition = resolver.file(path, &BTreeMap::new(), &BTreeMap::new(), 0)?;
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
    let definition = resolver.file(path, &BTreeMap::new(), &BTreeMap::new(), 0)?;
    Ok(ResolvedWorkflow {
        definition,
        sources: resolver.sources,
    })
}

pub fn resolve_flows(
    definition: &WorkflowDefinition,
    directory: Option<&Path>,
) -> Result<ResolvedWorkflow, FlowResolutionError> {
    if directory.is_none() && !needs_resolution(definition) {
        return Ok(ResolvedWorkflow {
            definition: definition.clone(),
            sources: BTreeSet::new(),
        });
    }
    let mut resolver = Resolver::default();
    let mut definition = definition.clone();
    resolver.definition(
        &mut definition,
        directory,
        &BTreeMap::new(),
        0,
        "workflow",
        &BTreeMap::new(),
    )?;
    resolver.finish(&mut definition);
    Ok(ResolvedWorkflow {
        definition,
        sources: resolver.sources,
    })
}

fn needs_resolution(definition: &WorkflowDefinition) -> bool {
    !definition.subgraphs.is_empty() || definition.nodes.iter().any(node_needs_resolution)
}

fn wire_node_needs_resolution(node: &serde_json::Value) -> bool {
    node.get("subgraphs")
        .is_some_and(|value| value.as_object().is_some_and(|value| !value.is_empty()))
        || node["kind"] == CALL_KIND
        || [
            node.pointer("/loop/body/nodes"),
            node.pointer("/config/body/nodes"),
        ]
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_array)
        .flatten()
        .any(wire_node_needs_resolution)
}

fn node_needs_resolution(node: &NodeDefinition) -> bool {
    !node.subgraphs.is_empty()
        || node.kind == CALL_KIND
        || node
            .loop_definition
            .as_ref()
            .is_some_and(|body| body.body.nodes.iter().any(node_needs_resolution))
        || (node.kind == mf_runtime::ITERATION_KIND
            && node
                .config
                .pointer("/body/nodes")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|nodes| nodes.iter().any(wire_node_needs_resolution)))
}

fn argument_descriptions(
    arguments: &BTreeMap<String, SubgraphSource>,
) -> BTreeMap<String, Vec<mf_telemetry::description::SubgraphArgumentDescription>> {
    fn source(value: &SubgraphSource) -> &SubgraphSource {
        value.innermost()
    }
    let describe = |name: &str, value: &SubgraphSource| {
        mf_telemetry::description::SubgraphArgumentDescription {
            name: name.to_owned(),
            kind: source(value)
                .inline_node_definition()
                .map_or_else(|| CALL_KIND.to_owned(), |node| node.kind.clone()),
        }
    };
    arguments
        .iter()
        .map(|(parameter, value)| {
            let entries = match source(value) {
                SubgraphSource::Collection { collection } => collection
                    .iter()
                    .map(|(name, value)| describe(name, value))
                    .collect(),
                value => vec![describe(parameter, value)],
            };
            (parameter.clone(), entries)
        })
        .collect()
}

impl Resolver {
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
        bindings: &BTreeMap<String, SubgraphSource>,
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
            bindings,
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
        bindings: &BTreeMap<String, SubgraphSource>,
    ) -> Result<(), FlowResolutionError> {
        let resolve = needs_resolution(definition) || !bindings.is_empty();
        ensure!(
            depth <= MAX_CALL_DEPTH,
            InvalidSnafu {
                reference,
                message: "Flow nesting exceeds sixteen calls"
            }
        );
        ensure!(
            definition.subgraphs.is_empty() || definition.version.supports_nested_flows(),
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
        let mut flows = std::mem::take(&mut definition.subgraphs);
        for name in bindings.keys() {
            ensure!(
                matches!(flows.get(name), Some(SubgraphSource::Parameter { .. })),
                InvalidSnafu {
                    reference: name,
                    message: "unknown subgraph parameter"
                }
            );
        }
        let mut declarations = Vec::new();
        for (name, source) in &mut flows {
            ensure!(
                !name.trim().is_empty(),
                InvalidSnafu {
                    reference: name,
                    message: "subgraph names must not be blank"
                }
            );
            if let SubgraphSource::Parameter { parameter, default } = source {
                let selected = bindings
                    .get(name)
                    .cloned()
                    .or_else(|| default.as_deref().cloned())
                    .context(InvalidSnafu {
                        reference: name,
                        message: "required subgraph parameter was not provided",
                    })?;
                declarations.push((name.clone(), *parameter));
                *source = selected;
            }
        }
        for (name, kind) in declarations {
            let source = self.source(
                &flows[&name],
                &flows,
                directory,
                &available,
                depth,
                &BTreeMap::new(),
            )?;
            let cardinality = source.innermost();
            ensure!(
                matches!(cardinality, ResolvedSubgraphSource::Collection { .. })
                    == (kind == mf_runtime::SubgraphParameterKind::Collection),
                InvalidSnafu {
                    reference: &name,
                    message: "subgraph parameter cardinality does not match its declaration"
                }
            );
            flows.insert(name, source.into_source());
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

    /// Resolves an author-facing source into one of the normalized `Inline`, `Collection`, or
    /// `View` shapes. Paths, built-ins, references, parameters, and bindings do not survive this
    /// boundary, so call lowering and freezing never depend on the original source kind.
    fn source(
        &mut self,
        source: &SubgraphSource,
        scope: &BTreeMap<String, SubgraphSource>,
        directory: Option<&Path>,
        available: &BTreeMap<String, NodeDependency>,
        depth: usize,
        bindings: &BTreeMap<String, SubgraphSource>,
    ) -> Result<ResolvedSubgraphSource, FlowResolutionError> {
        ensure!(
            depth <= MAX_CALL_DEPTH,
            InvalidSnafu {
                reference: "subgraph",
                message: "subgraph reference depth exceeds sixteen levels",
            }
        );
        let mut definition = match source {
            SubgraphSource::Reference { reference } => {
                let source = scope.get(reference).context(InvalidSnafu {
                    reference,
                    message: "unknown subgraph parameter or source",
                })?;
                return self.source(source, scope, directory, available, depth + 1, bindings);
            }
            SubgraphSource::View { source, inputs } => {
                let source =
                    self.source(source, scope, directory, available, depth + 1, bindings)?;
                ensure!(
                    !matches!(
                        source.innermost(),
                        ResolvedSubgraphSource::Collection { .. }
                    ),
                    InvalidSnafu {
                        reference: "view",
                        message: "an input projection requires one subgraph"
                    }
                );
                return Ok(ResolvedSubgraphSource::view(
                    Box::new(source),
                    inputs.clone(),
                ));
            }
            SubgraphSource::Collection { collection } => {
                ensure!(
                    bindings.is_empty(),
                    InvalidSnafu {
                        reference: "collection",
                        message: "a collection cannot be called as one Flow"
                    }
                );
                let mut resolved = BTreeMap::new();
                for (name, source) in collection {
                    ensure!(
                        !name.trim().is_empty() && !name.contains(':'),
                        InvalidSnafu {
                            reference: name,
                            message: "subgraph names must be nonblank and cannot contain colons"
                        }
                    );
                    let source = self.source(
                        source,
                        scope,
                        directory,
                        available,
                        depth + 1,
                        &BTreeMap::new(),
                    )?;
                    ensure!(
                        !matches!(source, ResolvedSubgraphSource::Collection { .. }),
                        InvalidSnafu {
                            reference: name,
                            message: "subgraph collections contain individual callable subgraphs"
                        }
                    );
                    resolved.insert(name.clone(), source);
                }
                return Ok(ResolvedSubgraphSource::Collection {
                    collection: resolved,
                });
            }
            SubgraphSource::Source(SubgraphSourceKind::Inline {
                node: Some(node),
                definition: None,
            }) => {
                let mut node = node.clone();
                node.subgraphs.extend(bindings.clone());
                self.nodes(
                    std::slice::from_mut(&mut node),
                    scope,
                    directory,
                    available,
                    depth + 1,
                    mf_runtime::WorkflowDefinitionVersion::CURRENT,
                )?;
                let definition = WorkflowDefinition {
                    version: mf_runtime::WorkflowDefinitionVersion::CURRENT,
                    execution: None,
                    dependencies: available.clone(),
                    subgraphs: Default::default(),
                    nodes: vec![*node.clone()],
                    edges: vec![],
                    control_edges: vec![],
                    outputs: vec![],
                    order_edges: vec![],
                    clusters: vec![],
                };
                return Ok(ResolvedSubgraphSource::inline_node(
                    node,
                    Box::new(definition),
                ));
            }
            SubgraphSource::Source(SubgraphSourceKind::Inline {
                definition: Some(definition),
                node: None,
            }) => *definition.clone(),
            SubgraphSource::Source(SubgraphSourceKind::Inline { .. }) => {
                return InvalidSnafu {
                    reference: "inline",
                    message: "inline subgraphs require exactly one of node or definition",
                }
                .fail();
            }
            SubgraphSource::Parameter { .. } => {
                return InvalidSnafu {
                    reference: "parameter",
                    message: "unbound subgraph parameter",
                }
                .fail();
            }
            SubgraphSource::Source(SubgraphSourceKind::Path { path }) => {
                let path = if path.is_absolute() {
                    path.clone()
                } else {
                    directory
                        .context(InvalidSnafu {
                            reference: path.display().to_string(),
                            message: "relative subgraph sources require a source directory",
                        })?
                        .join(path)
                };
                let definition = self.file(&path, available, bindings, depth + 1)?;
                return Ok(ResolvedSubgraphSource::inline(Box::new(definition)));
            }
            SubgraphSource::Source(SubgraphSourceKind::Builtin { builtin }) => {
                let registry = mf_runtime::BuiltinFlowRegistry::from_inventory()?;
                let registration = registry.get(builtin).context(InvalidSnafu {
                    reference: builtin,
                    message: "unknown built-in Flow; link its provider before resolving sources",
                })?;
                let mut definition = (registration.definition)().context(ParseSnafu {
                    path: PathBuf::from(format!("builtin:{builtin}")),
                })?;
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
                self.definition(
                    &mut definition,
                    None,
                    available,
                    depth + 1,
                    builtin,
                    bindings,
                )?;
                return Ok(ResolvedSubgraphSource::inline(Box::new(definition)));
            }
        };
        self.definition(
            &mut definition,
            directory,
            available,
            depth + 1,
            "subgraph",
            bindings,
        )?;
        Ok(ResolvedSubgraphSource::inline(Box::new(definition)))
    }

    fn nodes(
        &mut self,
        nodes: &mut [NodeDefinition],
        sources: &BTreeMap<String, SubgraphSource>,
        directory: Option<&Path>,
        available: &BTreeMap<String, NodeDependency>,
        depth: usize,
        version: mf_runtime::WorkflowDefinitionVersion,
    ) -> Result<(), FlowResolutionError> {
        for node in nodes {
            let mut arguments: BTreeMap<String, SubgraphSource> = BTreeMap::new();
            for (name, source) in std::mem::take(&mut node.subgraphs) {
                ensure!(
                    !name.trim().is_empty() && !name.contains(':'),
                    InvalidSnafu {
                        reference: &name,
                        message: "subgraph parameter names must be nonblank and cannot contain colons"
                    }
                );
                arguments.insert(
                    name,
                    self.source(
                        &source,
                        sources,
                        directory,
                        available,
                        depth + 1,
                        &BTreeMap::new(),
                    )?
                    .into_source(),
                );
            }
            // Freezing adapts Nodes into Flows; retain the original caller-facing identities.
            if node.subgraph_arguments.is_empty() && !arguments.is_empty() {
                node.subgraph_arguments = argument_descriptions(&arguments);
            }
            if node.kind == CALL_KIND {
                let mut config: CallConfig =
                    serde_json::from_value(node.config.clone()).context(ConfigurationSnafu {
                        node: node.id.as_str(),
                    })?;
                ensure!(
                    version.supports_nested_flows(),
                    InvalidSnafu {
                        reference: &config.flow,
                        message: "Flow calls require version 2026-10-25 or later"
                    }
                );
                ensure!(
                    !config.flow.trim().is_empty(),
                    InvalidSnafu {
                        reference: &config.flow,
                        message: "Flow alias must not be blank"
                    }
                );
                let source = if let Some(definition) = config.definition.take() {
                    SubgraphSource::inline(definition)
                } else {
                    sources
                        .get(&config.flow)
                        .context(InvalidSnafu {
                            reference: &config.flow,
                            message: "unknown Flow alias",
                        })?
                        .clone()
                };
                let resolved =
                    self.source(&source, sources, directory, available, depth, &arguments)?;
                let Some((definition, node_source)) = resolved.frozen_definition() else {
                    return InvalidSnafu {
                        reference: &config.flow,
                        message: "Flow calls require one callable subgraph",
                    }
                    .fail();
                };
                fn projections(
                    source: &ResolvedSubgraphSource,
                    output: &mut Vec<BTreeMap<String, String>>,
                ) {
                    if let ResolvedSubgraphSource::View { source, inputs } = source {
                        projections(source, output);
                        output.push(inputs.clone());
                    }
                }
                if node_source {
                    config.node_source = true;
                }
                let mut inputs = Vec::new();
                projections(&resolved, &mut inputs);
                if !inputs.is_empty() {
                    config.input_projections = inputs;
                }
                let mut definition = Box::new(definition);
                ensure!(
                    definition.execution.is_none() || version.supports_child_streams(),
                    InvalidSnafu {
                        reference: &config.flow,
                        message: "Flow calls require a terminal child interface"
                    }
                );
                definition.subgraphs.clear();
                config.definition = Some(definition);
                node.config = serde_json::to_value(config).context(ConfigurationSnafu {
                    node: node.id.as_str(),
                })?;
            } else {
                node.subgraphs = arguments;
            }
            if let Some(definition) = &mut node.loop_definition {
                self.nodes(
                    &mut definition.body.nodes,
                    sources,
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
                    sources,
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
