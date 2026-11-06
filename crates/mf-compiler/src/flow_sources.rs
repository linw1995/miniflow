use crate::{NodeDependency, WorkflowDefinition};
use mf_runtime::{
    CALL_KIND, DefinitionId, FlowSource, MAX_CALL_DEPTH, NodeDefinition, NodeRegistry,
};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Snafu)]
pub enum FlowResolutionError {
    #[snafu(transparent)]
    BuiltinRegistry {
        source: mf_runtime::BuiltinFlowRegistryError,
    },
    #[snafu(transparent)]
    NodeRegistry {
        source: mf_runtime::NodeRegistryError,
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
    #[snafu(display("invalid Flow configuration on `{node}`: {source}"))]
    Configuration {
        node: String,
        source: serde_json::Error,
    },
    #[snafu(display("invalid child scope on `{node}`: {source}"))]
    ChildStructure {
        node: String,
        source: crate::ScopeStructureError,
    },
    #[snafu(display("invalid Flow reference `{reference}`: {message}"))]
    Invalid { reference: String, message: String },
    #[snafu(display("conflicting provider sources for package `{package}` in Flow `{reference}`"))]
    Dependency { package: String, reference: String },
}

/// Private authoring codec, applied only after a receiving node declares a field.
/// Ordinary JSON is never inspected here.
#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum ParameterSource {
    Source(FlowSource),
    Definition { definition: Box<WorkflowDefinition> },
    Resolved(WorkflowDefinition),
}

/// A complete host-side resolution result. Partial provider-discovery handoffs
/// deliberately use FlowDiscovery rather than this type. Root sources aggregate
/// the entire resolution; child scopes retain only their own source provenance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedWorkflow {
    pub definition: WorkflowDefinition,
    pub sources: BTreeSet<PathBuf>,
    pub children: BTreeMap<DefinitionId, BTreeMap<String, Self>>,
}

/// Build-product handoff: authored UTF-8 resources and a package-relative entry.
/// It contains neither receiver callbacks nor a partially resolved workflow.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowSourceBundle {
    #[serde(serialize_with = "serialize_embedded_path")]
    pub entry: PathBuf,
    #[serde(serialize_with = "serialize_embedded_sources")]
    pub sources: BTreeMap<PathBuf, String>,
}

fn portable_embedded_path(path: &Path) -> Result<String, &'static str> {
    path.components()
        .map(|component| match component {
            Component::Prefix(_) | Component::RootDir => {
                Err("embedded Flow source path must be relative to the manifest directory")
            }
            component => component
                .as_os_str()
                .to_str()
                .ok_or("embedded Flow source path must be valid UTF-8"),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|components| components.join("/"))
}

fn serialize_embedded_path<S: serde::Serializer>(
    path: &Path,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let path = portable_embedded_path(path).map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(&path)
}

fn serialize_embedded_sources<S: serde::Serializer>(
    sources: &BTreeMap<PathBuf, String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;

    let mut map = serializer.serialize_map(Some(sources.len()))?;
    for (path, source) in sources {
        let path = portable_embedded_path(path).map_err(serde::ser::Error::custom)?;
        map.serialize_entry(&path, source)?;
    }
    map.end()
}

impl FlowSourceBundle {
    /// Resolve host resources without interpreting their provider dependency paths.
    pub fn resolve(
        &self,
        registry: &NodeRegistry,
    ) -> Result<ResolvedWorkflow, FlowResolutionError> {
        let mut sources = BTreeMap::new();
        for (path, source) in &self.sources {
            let normalized = normalize_embedded_path(path)?;
            ensure!(
                sources.insert(normalized, source.clone()).is_none(),
                InvalidSnafu {
                    reference: path.display().to_string(),
                    message: "duplicate normalized embedded Flow source path",
                }
            );
        }
        let mut resolver = Resolver {
            host_dependencies: true,
            embedded_sources: Some(sources),
            extractors: registry.flow_extractors(),
            ..Default::default()
        };
        let definition = resolver.file(&self.entry, &BTreeMap::new(), 0)?;
        Ok(resolver.finish_resolution(definition))
    }
}

/// Normalize a manifest-relative archive identity without consulting the filesystem.
pub fn normalize_embedded_path(path: &Path) -> Result<PathBuf, FlowResolutionError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(component) => normalized.push(component),
            Component::CurDir => {}
            Component::ParentDir => {
                ensure!(
                    normalized.pop(),
                    InvalidSnafu {
                        reference: path.display().to_string(),
                        message: "embedded Flow source path escapes the manifest directory",
                    }
                );
            }
            Component::Prefix(_) | Component::RootDir => {
                return InvalidSnafu {
                    reference: path.display().to_string(),
                    message: "embedded Flow source path must be relative to the manifest directory",
                }
                .fail();
            }
        }
    }
    ensure!(
        !normalized.as_os_str().is_empty(),
        InvalidSnafu {
            reference: path.display().to_string(),
            message: "embedded Flow source path must name a file",
        }
    );
    Ok(normalized)
}

/// Metadata-only discovery may pause at a provider that is not linked yet.
/// Consumers must retry from the authored source with the expanded provider set;
/// a result with pending entries is not an executable or structurally frozen plan.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowDiscovery {
    /// Definition carrying selected provider dependencies. Children contain actual
    /// scopes confirmed by receiver callbacks, never pending source placeholders.
    /// Root sources/pending aggregate descendants; children keep scope-local facts.
    pub definition: WorkflowDefinition,
    pub sources: BTreeSet<PathBuf>,
    pub pending: BTreeSet<String>,
    pub children: BTreeMap<DefinitionId, BTreeMap<String, Self>>,
}

type FlowExtractor = fn(&serde_json::Value) -> BTreeMap<String, serde_json::Value>;
type ScopeChildren = BTreeMap<DefinitionId, BTreeMap<String, Scope>>;

/// A scope captured during normalization, not rediscovered from normalized JSON.
/// Only strict entrypoints turn these scopes into a ResolvedWorkflow.
#[derive(Debug)]
struct Scope {
    definition: WorkflowDefinition,
    sources: BTreeSet<PathBuf>,
    pending: BTreeSet<String>,
    children: ScopeChildren,
}

impl Scope {
    fn into_resolved(self) -> ResolvedWorkflow {
        ResolvedWorkflow {
            definition: self.definition,
            sources: self.sources,
            children: self
                .children
                .into_iter()
                .map(|(node, fields)| {
                    (
                        node,
                        fields
                            .into_iter()
                            .map(|(field, child)| (field, child.into_resolved()))
                            .collect(),
                    )
                })
                .collect(),
        }
    }

    fn into_discovery(self) -> FlowDiscovery {
        FlowDiscovery {
            definition: self.definition,
            sources: self.sources,
            pending: self.pending,
            children: self
                .children
                .into_iter()
                .map(|(node, fields)| {
                    (
                        node,
                        fields
                            .into_iter()
                            .map(|(field, child)| (field, child.into_discovery()))
                            .collect(),
                    )
                })
                .collect(),
        }
    }
}

#[derive(Default)]
struct Resolver {
    host_dependencies: bool,
    embedded_sources: Option<BTreeMap<PathBuf, String>>,
    discovery: bool,
    unlinked: bool,
    extractors: BTreeMap<String, FlowExtractor>,
    pending_scopes: Vec<BTreeSet<String>>,
    builtin_catalog: BTreeMap<String, WorkflowDefinition>,
    selected_packages: BTreeSet<String>,
    pending: BTreeSet<String>,
    stack: Vec<PathBuf>,
    scope_depth: usize,
    sources: BTreeSet<PathBuf>,
    dependencies: BTreeMap<String, NodeDependency>,
}

pub fn load_workflow(path: &Path) -> Result<ResolvedWorkflow, FlowResolutionError> {
    load_workflow_with_registry(path, &NodeRegistry::from_inventory()?)
}

pub fn load_workflow_with_registry(
    path: &Path,
    registry: &NodeRegistry,
) -> Result<ResolvedWorkflow, FlowResolutionError> {
    let mut resolver = Resolver {
        extractors: registry.flow_extractors(),
        ..Default::default()
    };
    let definition = resolver.file(path, &BTreeMap::new(), 0)?;
    Ok(resolver.finish_resolution(definition))
}

/// Loads Flow files while the embedding host retains authority over provider dependencies.
pub fn load_embedded_workflow(path: &Path) -> Result<ResolvedWorkflow, FlowResolutionError> {
    let registry = NodeRegistry::from_inventory()?;
    let mut resolver = Resolver {
        host_dependencies: true,
        extractors: registry.flow_extractors(),
        ..Default::default()
    };
    let definition = resolver.file(path, &BTreeMap::new(), 0)?;
    Ok(resolver.into_resolved(definition))
}

pub fn resolve_flows(
    definition: &WorkflowDefinition,
    directory: Option<&Path>,
) -> Result<ResolvedWorkflow, FlowResolutionError> {
    resolve_flows_with_registry(definition, directory, &NodeRegistry::from_inventory()?)
}

pub fn resolve_flows_with_registry(
    definition: &WorkflowDefinition,
    directory: Option<&Path>,
    registry: &NodeRegistry,
) -> Result<ResolvedWorkflow, FlowResolutionError> {
    let mut resolver = Resolver {
        extractors: registry.flow_extractors(),
        ..Default::default()
    };
    let mut definition = definition.clone();
    let scope = resolver.definition(&mut definition, directory, &BTreeMap::new(), 0, "workflow")?;
    Ok(resolver.finish_resolution(scope))
}

/// Resolve everything whose linked metadata is available, without executing factories.
/// Unknown providers and builtins are explicit pending entries, never guessed from JSON.
pub fn discover_flows_with_registry(
    definition: &WorkflowDefinition,
    directory: Option<&Path>,
    registry: &NodeRegistry,
) -> Result<FlowDiscovery, FlowResolutionError> {
    let resolver = Resolver {
        discovery: true,
        extractors: registry.flow_extractors(),
        ..Default::default()
    };
    resolve_using(resolver, definition, directory)
}

/// Initial CLI discovery has no authority from providers linked into the CLI host.
#[cfg(test)]
fn discover_unlinked_flows(
    definition: &WorkflowDefinition,
    directory: Option<&Path>,
) -> Result<FlowDiscovery, FlowResolutionError> {
    resolve_using(
        Resolver {
            discovery: true,
            unlinked: true,
            ..Default::default()
        },
        definition,
        directory,
    )
}

fn resolve_using(
    mut resolver: Resolver,
    definition: &WorkflowDefinition,
    directory: Option<&Path>,
) -> Result<FlowDiscovery, FlowResolutionError> {
    let mut definition = definition.clone();
    let scope = resolver.definition(&mut definition, directory, &BTreeMap::new(), 0, "workflow")?;
    Ok(resolver.finish_discovery(scope))
}

impl FlowDiscovery {
    /// Attribute exhausted node discovery using only the already authorized
    /// child tree. Pending source values (notably builtins) are
    /// not frozen children and retain the pipeline's pending-source diagnostic.
    #[cfg(feature = "codegen")]
    pub fn check_pending_nodes(&self) -> Result<(), crate::WorkflowCompileError> {
        for node in &self.definition.nodes {
            if self.pending.contains(&format!("node:{}", node.kind)) {
                return Err(crate::WorkflowCompileError::UnknownNodeKind {
                    definition_id: node.id.clone(),
                    kind: node.kind.clone(),
                });
            }
            for child in self
                .children
                .get(&node.id)
                .into_iter()
                .flat_map(BTreeMap::values)
            {
                crate::calls::child_result(node, child.check_pending_nodes())?;
            }
        }
        Ok(())
    }

    /// Discover from an authored file, retaining its canonical recursion frame.
    pub fn load_with_registry(
        path: &Path,
        registry: &NodeRegistry,
    ) -> Result<Self, FlowResolutionError> {
        Self::load_with_builtin_catalog(path, registry, BTreeMap::new(), BTreeSet::new())
    }

    /// Fully resolve authored sources once after the provider closure is stable.
    /// This uses the selected process registry, never the CLI host's node metadata.
    pub fn resolve_with_builtin_catalog(
        path: &Path,
        registry: &NodeRegistry,
        builtin_catalog: BTreeMap<String, WorkflowDefinition>,
        selected_packages: BTreeSet<String>,
    ) -> Result<ResolvedWorkflow, FlowResolutionError> {
        let mut resolver = Resolver {
            extractors: registry.flow_extractors(),
            builtin_catalog,
            selected_packages,
            ..Default::default()
        };
        let definition = resolver.file(path, &BTreeMap::new(), 0)?;
        Ok(resolver.finish_resolution(definition))
    }

    /// Replay authored sources with an application-owned static builtin catalog.
    /// Selected providers retain exclusive authority over their own namespaces;
    /// this catalog never supplies receiver callbacks.
    pub fn load_with_builtin_catalog(
        path: &Path,
        registry: &NodeRegistry,
        builtin_catalog: BTreeMap<String, WorkflowDefinition>,
        selected_packages: BTreeSet<String>,
    ) -> Result<Self, FlowResolutionError> {
        discover_file(
            path,
            Resolver {
                discovery: true,
                extractors: registry.flow_extractors(),
                builtin_catalog,
                selected_packages,
                ..Default::default()
            },
        )
    }
}

#[cfg(feature = "codegen")]
pub fn discover_unlinked_workflow(path: &Path) -> Result<FlowDiscovery, FlowResolutionError> {
    discover_unlinked_with_catalog(path, &BTreeMap::new())
}

#[cfg(feature = "codegen")]
pub fn discover_unlinked_with_catalog(
    path: &Path,
    builtin_catalog: &BTreeMap<String, WorkflowDefinition>,
) -> Result<FlowDiscovery, FlowResolutionError> {
    discover_file(
        path,
        Resolver {
            discovery: true,
            unlinked: true,
            builtin_catalog: builtin_catalog.clone(),
            ..Default::default()
        },
    )
}

/// Snapshot only named, explicitly requested public catalog entries. Definitions
/// are static metadata; node factories and their configuration roles are absent.
#[cfg(feature = "codegen")]
pub fn snapshot_pending_builtins(
    pending: &BTreeSet<String>,
    selected_packages: &BTreeSet<String>,
    catalog: &mut BTreeMap<String, WorkflowDefinition>,
) -> Result<bool, FlowResolutionError> {
    let requested: Vec<_> = pending
        .iter()
        .filter_map(|entry| entry.strip_prefix("builtin:"))
        .filter(|name| {
            !catalog.contains_key(*name)
                && builtin_owner(name).is_none_or(|owner| !selected_packages.contains(owner))
        })
        .collect();
    if requested.is_empty() {
        return Ok(false);
    }
    let registry = mf_runtime::BuiltinFlowRegistry::from_inventory()?;
    let mut changed = false;
    for name in requested {
        if let Some(registration) = registry.get(name) {
            let definition = (registration.definition)().context(ParseSnafu {
                path: PathBuf::from(format!("builtin:{name}")),
            })?;
            catalog.insert(name.to_owned(), definition);
            changed = true;
        }
    }
    Ok(changed)
}

fn builtin_owner(name: &str) -> Option<&str> {
    name.rsplit_once('.').map(|(package, _)| package)
}

fn discover_file(
    path: &Path,
    mut resolver: Resolver,
) -> Result<FlowDiscovery, FlowResolutionError> {
    let scope = resolver.file(path, &BTreeMap::new(), 0)?;
    Ok(resolver.finish_discovery(scope))
}

impl Resolver {
    fn finish_resolution(self, mut scope: Scope) -> ResolvedWorkflow {
        self.finish(&mut scope.definition);
        self.into_resolved(scope)
    }

    fn into_resolved(self, mut scope: Scope) -> ResolvedWorkflow {
        debug_assert!(!self.discovery);
        scope.sources = self.sources;
        scope.into_resolved()
    }

    fn finish_discovery(self, mut scope: Scope) -> FlowDiscovery {
        self.finish(&mut scope.definition);
        scope.sources = self.sources;
        scope.pending = self.pending;
        scope.into_discovery()
    }

    fn mark_pending(&mut self, entry: String) {
        self.pending.insert(entry.clone());
        if let Some(scope) = self.pending_scopes.last_mut() {
            scope.insert(entry);
        }
    }

    fn with_scope<T>(
        &mut self,
        reference: &str,
        resolve: impl FnOnce(&mut Self) -> Result<T, FlowResolutionError>,
    ) -> Result<T, FlowResolutionError> {
        ensure!(
            self.scope_depth < mf_telemetry::MAX_SCOPE_DEPTH,
            InvalidSnafu {
                reference,
                message: "combined container nesting exceeds twenty scopes",
            }
        );
        self.scope_depth += 1;
        let result = resolve(self);
        self.scope_depth -= 1;
        result
    }

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
    ) -> Result<Scope, FlowResolutionError> {
        let path = if self.embedded_sources.is_some() {
            normalize_embedded_path(path)?
        } else {
            fs::canonicalize(path).context(ReadSnafu { path })?
        };
        ensure!(
            !self.stack.contains(&path),
            InvalidSnafu {
                reference: path.display().to_string(),
                message: format!("recursive Flow reference through {:?}", self.stack)
            }
        );
        let source = if let Some(sources) = &self.embedded_sources {
            sources
                .get(&path)
                .cloned()
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "Flow source is not embedded")
                })
                .context(ReadSnafu { path: &path })?
        } else {
            fs::read_to_string(&path).context(ReadSnafu { path: &path })?
        };
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
        let mut scope = result.context(SourceSnafu { path: &path })?;
        scope.sources.insert(path);
        Ok(scope)
    }

    fn definition(
        &mut self,
        definition: &mut WorkflowDefinition,
        directory: Option<&Path>,
        inherited: &BTreeMap<String, NodeDependency>,
        depth: usize,
        reference: &str,
    ) -> Result<Scope, FlowResolutionError> {
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
            if !self.host_dependencies {
                self.merge(dependency, reference)?;
            }
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
        self.pending_scopes.push(BTreeSet::new());
        let result = self.nodes(
            &mut definition.nodes,
            &flows,
            directory,
            &available,
            depth,
            definition.version,
        );
        let pending = self.pending_scopes.pop().expect("active definition scope");
        Ok(Scope {
            definition: definition.clone(),
            sources: BTreeSet::new(),
            pending,
            children: result?,
        })
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
        reference: &str,
    ) -> Result<Option<Scope>, FlowResolutionError> {
        match source {
            FlowSource::Path { path } => {
                let directory = directory.with_context(|| InvalidSnafu {
                    reference,
                    message: "local Flow references require load_workflow or a source directory",
                })?;
                Ok(Some(self.file(&directory.join(path), available, depth)?))
            }
            FlowSource::Builtin { builtin } => {
                let registry = if self.unlinked {
                    None
                } else {
                    Some(mf_runtime::BuiltinFlowRegistry::from_inventory()?)
                };
                let registration = registry.as_ref().and_then(|registry| registry.get(builtin));
                let fallback = if builtin_owner(builtin).is_some_and(|owner| {
                    self.selected_packages.contains(owner)
                        || available.contains_key(owner)
                        || self.dependencies.contains_key(owner)
                }) {
                    None
                } else {
                    self.builtin_catalog.get(builtin).cloned()
                };
                let mut definition = if let Some(registration) = registration {
                    (registration.definition)().context(ParseSnafu {
                        path: PathBuf::from(format!("builtin:{builtin}")),
                    })?
                } else if let Some(definition) = fallback {
                    definition
                } else {
                    if self.discovery {
                        self.mark_pending(format!("builtin:{builtin}"));
                        return Ok(None);
                    }
                    return InvalidSnafu {
                        reference: builtin,
                        message: "unknown built-in Flow; link its provider before resolving sources",
                    }.fail();
                };
                // Builtins carry package identities, but the caller selects actual provider sources.
                for dependency in definition.dependencies.values_mut() {
                    if let Some(selected) = available.get(&dependency.package) {
                        let features = std::mem::take(&mut dependency.features);
                        let default_features = dependency.default_features;
                        *dependency = selected.clone();
                        dependency.features.extend(features);
                        dependency.features.sort();
                        dependency.features.dedup();
                        dependency.default_features |= default_features;
                    }
                }
                self.definition(&mut definition, None, available, depth, builtin)
                    .map(Some)
            }
        }
    }

    fn parameter(
        &mut self,
        parameter: serde_json::Value,
        directory: Option<&Path>,
        available: &BTreeMap<String, NodeDependency>,
        depth: usize,
        reference: &str,
    ) -> Result<Option<Scope>, FlowResolutionError> {
        self.with_scope(reference, |resolver| {
            let source: ParameterSource = serde_json::from_value(parameter)
                .context(ConfigurationSnafu { node: reference })?;
            let mut definition = match source {
                ParameterSource::Source(source) => {
                    return resolver.source(&source, directory, available, depth, reference);
                }
                ParameterSource::Definition { definition } => *definition,
                ParameterSource::Resolved(definition) => definition,
            };
            resolver
                .definition(&mut definition, directory, available, depth, reference)
                .map(Some)
        })
    }

    fn parameters(
        &mut self,
        node: &mut NodeDefinition,
        directory: Option<&Path>,
        available: &BTreeMap<String, NodeDependency>,
        depth: usize,
        version: mf_runtime::WorkflowDefinitionVersion,
    ) -> Result<BTreeMap<String, Scope>, FlowResolutionError> {
        let Some(extractor) = self.extractors.get(&node.kind) else {
            // Only the selected receiving provider grants source authority.
            return Ok(BTreeMap::new());
        };
        let parameters = extractor(&node.config);
        let mut children = BTreeMap::new();
        for field in parameters.keys() {
            ensure!(
                !field.trim().is_empty(),
                InvalidSnafu {
                    reference: node.id.as_str(),
                    message: format!(
                        "blank configuration parameter declaration on `{}`",
                        node.kind
                    )
                }
            );
        }
        for (field, value) in &parameters {
            let reference = format!("{}.config.{field}", node.id);
            // Validate an authorized inline Iteration boundary before descending
            // into its nodes, so malformed nested containers cannot obscure the
            // owner's typed policy error. Source-backed bodies use the same codec
            // and are validated after resolution below.
            if node.kind == mf_runtime::ITERATION_KIND && field == "body" {
                let parameter: ParameterSource = serde_json::from_value(value.clone())
                    .context(ConfigurationSnafu { node: &reference })?;
                let body = match parameter {
                    ParameterSource::Definition { definition } => Some(*definition),
                    ParameterSource::Resolved(definition) => Some(definition),
                    ParameterSource::Source(_) => None,
                };
                if let Some(body) = body {
                    self.validate_iteration_body(node, &body)?;
                }
            }
            let call_body = node.kind == CALL_KIND && field == "body";
            let child_depth = if field == "body" && !crate::calls::call_like(node) {
                depth
            } else {
                depth + 1
            };
            let child =
                self.parameter(value.clone(), directory, available, child_depth, &reference)?;
            if let Some(child) = child {
                if call_body {
                    ensure!(
                        child.definition.execution.is_none() || version.supports_child_streams(),
                        InvalidSnafu {
                            reference: &reference,
                            message: "Flow calls require a terminal child interface"
                        }
                    );
                }
                node.config[field] = serde_json::to_value(&child.definition)
                    .context(ConfigurationSnafu { node: &reference })?;
                children.insert(field.clone(), child);
            }
        }
        if node.kind == mf_runtime::ITERATION_KIND
            && let Some(body) = children.get("body")
        {
            self.validate_iteration_body(node, &body.definition)?;
        }
        Ok(children)
    }

    fn validate_iteration_body(
        &self,
        node: &NodeDefinition,
        body: &WorkflowDefinition,
    ) -> Result<(), FlowResolutionError> {
        // Check supported scope grammar before descending, without decoding the
        // receiver's ordinary configuration or its result-interface policy.
        crate::iteration::validate_body_structure(body).context(ChildStructureSnafu {
            node: node.id.as_str(),
        })
    }

    fn nodes(
        &mut self,
        nodes: &mut [NodeDefinition],
        flows: &BTreeMap<String, FlowSource>,
        directory: Option<&Path>,
        available: &BTreeMap<String, NodeDependency>,
        depth: usize,
        version: mf_runtime::WorkflowDefinitionVersion,
    ) -> Result<ScopeChildren, FlowResolutionError> {
        let mut children = BTreeMap::new();
        for node in nodes {
            if node.kind == CALL_KIND {
                let explicit_body = node.config.get("body").is_some_and(|body| !body.is_null());
                // Validate the named selector without interpreting
                // the receiving field before its selected metadata authorizes it.
                #[derive(Deserialize)]
                struct CallSelector {
                    flow: String,
                }
                let config: CallSelector =
                    serde_json::from_value(node.config.clone()).context(ConfigurationSnafu {
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
                let extractor = self.extractors.get(&node.kind).copied();
                if extractor.is_none() && (explicit_body || self.embedded_sources.is_some()) {
                    // Archived sources always require selected receiving metadata;
                    // named aliases must not bootstrap dependency discovery in a host.
                    // An explicit receiving value likewise has no intrinsic authority.
                    if self.discovery {
                        self.mark_pending(format!("node:{}", node.kind));
                        continue;
                    }
                    return InvalidSnafu {
                        reference: node.id.as_str(),
                        message: "no selected provider metadata authorizes the Call `body` Flow parameter",
                    }.fail();
                }
                if !explicit_body {
                    let source = flows.get(&config.flow).with_context(|| InvalidSnafu {
                        reference: &config.flow,
                        message: "unknown Flow alias",
                    })?;
                    if extractor.is_none() {
                        if self.discovery {
                            self.mark_pending(format!("node:{}", node.kind));
                        }
                        // Named aliases may bootstrap dependency discovery before
                        // Call's receiving provider is selected. Explicit bodies
                        // never take this business-only source-selection path.
                        if let Some(child) = self.with_scope(&config.flow, |resolver| {
                            resolver.source(source, directory, available, depth + 1, &config.flow)
                        })? {
                            ensure!(
                                child.definition.execution.is_none()
                                    || version.supports_child_streams(),
                                InvalidSnafu {
                                    reference: &config.flow,
                                    message: "Flow calls require a terminal child interface"
                                }
                            );
                            // Bootstrap facts do not grant receiving authority:
                            // this definition is deliberately absent from children.
                            node.config["body"] = serde_json::to_value(child.definition).context(
                                ConfigurationSnafu {
                                    node: node.id.as_str(),
                                },
                            )?;
                        }
                        continue;
                    }
                    // Ask the receiver about the selected alias value without
                    // modifying the original configuration on authorization failure.
                    let mut candidate = node.config.clone();
                    candidate["body"] =
                        serde_json::to_value(source).context(ConfigurationSnafu {
                            node: node.id.as_str(),
                        })?;
                    ensure!(
                        extractor.expect("selected receiver")(&candidate).contains_key("body"),
                        InvalidSnafu {
                            reference: node.id.as_str(),
                            message: "selected Call provider does not declare the `body` Flow parameter",
                        }
                    );
                    node.config = candidate;
                } else if let Some(extractor) = extractor {
                    ensure!(
                        extractor(&node.config).contains_key("body"),
                        InvalidSnafu {
                            reference: node.id.as_str(),
                            message: "selected Call provider does not declare the `body` Flow parameter",
                        }
                    );
                }
            } else if self.discovery
                && !self.extractors.contains_key(&node.kind)
                && !matches!(
                    node.kind.as_str(),
                    mf_runtime::LOOP_ASSIGN_KIND
                        | mf_runtime::EXIT_LOOP_KIND
                        | mf_runtime::LOOP_SOURCE_ID
                        | mf_runtime::ITERATION_INPUT_KIND
                )
            {
                self.mark_pending(format!("node:{}", node.kind));
            }
            let fields = self.parameters(node, directory, available, depth, version)?;
            if !fields.is_empty() {
                children.insert(node.id.clone(), fields);
            }
        }
        Ok(children)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::CallConfig;
    use serde_json::json;

    fn definition(nodes: serde_json::Value) -> WorkflowDefinition {
        WorkflowDefinition::from_json(
            &json!({
                "version": "2026-10-28", "dependencies": {}, "nodes": nodes,
            })
            .to_string(),
        )
        .unwrap()
    }

    fn dependency(package: &str) -> NodeDependency {
        NodeDependency {
            package: package.into(),
            version: None,
            path: None,
            git: None,
            rev: None,
            features: vec![],
            default_features: false,
        }
    }

    fn source_definition(kind: &str, body: serde_json::Value) -> WorkflowDefinition {
        definition(json!([{"id":"owner", "kind":kind, "config":{"body":body}}]))
    }

    fn body_parameter(config: &serde_json::Value) -> BTreeMap<String, serde_json::Value> {
        config
            .get("body")
            .filter(|value| !value.is_null())
            .map(|body| BTreeMap::from([("body".into(), body.clone())]))
            .unwrap_or_default()
    }

    fn no_parameters(_: &serde_json::Value) -> BTreeMap<String, serde_json::Value> {
        BTreeMap::new()
    }

    fn source_resolver() -> Resolver {
        Resolver {
            extractors: BTreeMap::from([
                (
                    "test.flow-source-owner".into(),
                    body_parameter as FlowExtractor,
                ),
                ("test.ordinary".into(), no_parameters as FlowExtractor),
                (CALL_KIND.into(), body_parameter as FlowExtractor),
            ]),
            ..Default::default()
        }
    }

    impl Resolver {
        fn resolve(
            &mut self,
            definition: &mut WorkflowDefinition,
            directory: Option<&Path>,
        ) -> Result<Scope, FlowResolutionError> {
            self.definition(definition, directory, &BTreeMap::new(), 0, "test")
        }

        fn plan(
            mut self,
            definition: &WorkflowDefinition,
        ) -> Result<crate::CompiledWorkflow, crate::WorkflowCompileError> {
            let scope = self.resolve(&mut definition.clone(), None)?;
            crate::compiler::plan_resolved_workflow(&self.into_resolved(scope))
        }
    }

    fn write_definition(root: &Path, name: &str, value: &WorkflowDefinition) {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string(value).unwrap()).unwrap();
    }

    #[cfg(feature = "codegen")]
    #[test]
    fn exhausted_discovery_preserves_receiving_scope_and_ignores_ordinary_json() {
        let root = tempfile::tempdir().unwrap();
        let child = definition(json!([{"id":"fetch", "kind":"missing.fetch"}]));
        write_definition(root.path(), "child.json", &child);
        for (kind, body, authorized) in [
            ("test.flow-source-owner", json!({"path":"child.json"}), true),
            ("test.ordinary", json!(child), false),
        ] {
            let mut parent = definition(json!([
                {"id":"pending-builtin", "kind":CALL_KIND, "config":{"flow":"missing"}},
                {"id":"receiver", "kind":kind, "config":{
                    "body":body, "source":{"path":"must-not-read.json"},
                    "ordinary":{"version":"2026-10-28", "dependencies":{},
                        "nodes":[{"id":"not-a-node", "kind":"missing.fetch"}]}
                }}
            ]));
            parent.flows.insert(
                "missing".into(),
                FlowSource::Builtin {
                    builtin: "missing.template".into(),
                },
            );
            let discovery = resolve_using(
                Resolver {
                    discovery: true,
                    ..source_resolver()
                },
                &parent,
                authorized.then_some(root.path()),
            )
            .unwrap();
            let mut pending = BTreeSet::from(["builtin:missing.template".into()]);
            let mut sources = BTreeSet::new();
            if authorized {
                pending.insert("node:missing.fetch".into());
                sources.insert(fs::canonicalize(root.path().join("child.json")).unwrap());
            }
            assert_eq!(discovery.pending, pending);
            assert_eq!(discovery.sources, sources);
            // Metadata crosses the provider-probe process boundary before inspection.
            let mut discovery: FlowDiscovery =
                serde_json::from_slice(&serde_json::to_vec(&discovery).unwrap()).unwrap();
            if !authorized {
                discovery.check_pending_nodes().unwrap();
                continue;
            }
            // Reporting consumes confirmed children, never rescans config JSON
            // or requires provider callbacks after the handoff is deserialized.
            discovery.definition.nodes[1].config["body"] = json!({"path":"must-not-read.json"});
            let error = discovery.check_pending_nodes().unwrap_err();
            for expected in ["receiver", "fetch", "missing.fetch", "unknown kind"] {
                assert!(error.to_string().contains(expected), "{error}");
            }
            assert!(matches!(
                error,
                crate::WorkflowCompileError::ChildGraph { definition_id, source }
                    if definition_id.as_str() == "receiver"
                        && matches!(source.as_ref(), crate::WorkflowCompileError::UnknownNodeKind {
                            definition_id, kind
                        } if definition_id.as_str() == "fetch" && kind == "missing.fetch")
            ));
        }
    }

    fn call_definition(source: serde_json::Value) -> WorkflowDefinition {
        let mut parent = definition(json!([{
            "id":"call", "kind":CALL_KIND, "config":{"flow":"a"}
        }]));
        parent.version = mf_runtime::WorkflowDefinitionVersion::CURRENT;
        parent
            .flows
            .insert("a".into(), serde_json::from_value(source).unwrap());
        parent
    }

    fn selected_catalog_builtin() -> Result<WorkflowDefinition, crate::DefinitionParseError> {
        Ok(definition(
            json!([{"id":"selected", "kind":"test.ordinary"}]),
        ))
    }

    inventory::submit! {
        mf_runtime::BuiltinFlowRegistration::new("source-catalog-test", "example", selected_catalog_builtin)
    }

    #[test]
    fn public_catalog_fallback_never_supplies_host_node_configuration_roles() {
        let mut parent = call_definition(json!({"builtin":"public-catalog.example"}));
        let catalog_body = definition(json!([{
            "id":"opaque", "kind":"host-only.owner", "config":{"body":{"path":"never-read.json"}}
        }]));
        let mut resolver = Resolver {
            discovery: true,
            unlinked: true,
            builtin_catalog: BTreeMap::from([("public-catalog.example".into(), catalog_body)]),
            ..Default::default()
        };
        resolver.resolve(&mut parent, None).unwrap();
        assert_eq!(
            resolver.pending,
            BTreeSet::from(["node:host-only.owner".into(), format!("node:{CALL_KIND}")])
        );
        assert!(resolver.sources.is_empty());
        assert!(resolver.dependencies.is_empty());
        assert_eq!(
            parent.nodes[0].config["body"]["nodes"][0]["config"]["body"],
            json!({"path":"never-read.json"})
        );
    }

    #[test]
    fn selected_builtin_registry_wins_over_catalog_fallback() {
        let mut parent = call_definition(json!({"builtin":"source-catalog-test.example"}));
        let mut resolver = source_resolver();
        resolver.builtin_catalog.insert(
            "source-catalog-test.example".into(),
            definition(json!([
                {"id":"fallback", "kind":"test.ordinary"}
            ])),
        );
        resolver.resolve(&mut parent, None).unwrap();
        assert_eq!(parent.nodes[0].config["body"]["nodes"][0]["id"], "selected");
    }

    #[test]
    fn selected_namespace_missing_builtin_cannot_be_resurrected_from_host_catalog() {
        for global_selection in [false, true] {
            let mut parent = call_definition(json!({"builtin":"selected-provider.removed"}));
            if !global_selection {
                parent.dependencies.insert(
                    "selected".into(),
                    NodeDependency {
                        version: Some("1".into()),
                        ..dependency("selected-provider")
                    },
                );
            }
            let mut resolver = Resolver {
                discovery: true,
                unlinked: true,
                builtin_catalog: BTreeMap::from([(
                    "selected-provider.removed".into(),
                    definition(json!([])),
                )]),
                selected_packages: if global_selection {
                    BTreeSet::from(["selected-provider".into()])
                } else {
                    BTreeSet::new()
                },
                ..Default::default()
            };
            resolver.resolve(&mut parent, None).unwrap();
            assert_eq!(
                resolver.pending,
                BTreeSet::from([
                    "builtin:selected-provider.removed".into(),
                    format!("node:{CALL_KIND}")
                ])
            );
            assert!(parent.nodes[0].config.get("body").is_none());
        }
    }

    fn metadata_plain(
        _: serde_json::Value,
    ) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
        panic!("source discovery must not instantiate a node")
    }

    fn metadata_prepared(
        _: &str,
        _: serde_json::Value,
        _: BTreeMap<String, mf_runtime::PreparedSubgraph>,
    ) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
        panic!("source discovery must not prepare a node")
    }

    inventory::submit! {
        mf_runtime::NodeRegistration {
            kind:"test.source-api-owner",
            factory:mf_runtime::NodeFactory::Subgraph {
                flow_parameters: body_parameter,
                plain:metadata_plain, prepared:metadata_prepared,
            },
        }
    }

    fn embedded_archive(entry: &str, sources: &[(&str, WorkflowDefinition)]) -> FlowSourceBundle {
        FlowSourceBundle {
            entry: entry.into(),
            sources: sources
                .iter()
                .map(|(path, definition)| {
                    (
                        PathBuf::from(path),
                        serde_json::to_string(definition).unwrap(),
                    )
                })
                .collect(),
        }
    }

    fn source_cause(error: &FlowResolutionError) -> &FlowResolutionError {
        match error {
            FlowResolutionError::Source { source, .. } => source_cause(source),
            error => error,
        }
    }

    #[test]
    fn strict_resolution_entries_share_dependency_and_source_results() {
        let root = tempfile::tempdir().unwrap();
        let provider = |feature: &str| NodeDependency {
            version: Some("1".into()),
            features: vec![feature.into()],
            ..dependency("resolution-provider")
        };
        let mut child = definition(json!([]));
        child
            .dependencies
            .insert("child-provider".into(), provider("child"));
        let mut parent = source_definition("test.source-api-owner", json!({"path":"child.json"}));
        parent
            .dependencies
            .insert("root-provider".into(), provider("root"));
        write_definition(root.path(), "root.json", &parent);
        write_definition(root.path(), "child.json", &child);
        let entry = fs::canonicalize(root.path().join("root.json")).unwrap();
        let child_path = fs::canonicalize(root.path().join("child.json")).unwrap();
        let registry = NodeRegistry::from_inventory().unwrap();
        let file = load_workflow_with_registry(&entry, &registry).unwrap();
        assert_eq!(
            file.definition.dependencies["root-provider"].features,
            vec!["child", "root"]
        );
        assert_eq!(file.sources, BTreeSet::from([entry.clone(), child_path]));
        assert_eq!(file.children[&"owner".into()]["body"].definition, child);
        assert_eq!(
            file.children[&"owner".into()]["body"].sources,
            BTreeSet::from([fs::canonicalize(root.path().join("child.json")).unwrap()])
        );
        assert!(file.children[&"owner".into()]["body"].children.is_empty());
        assert_eq!(file.definition.nodes[0].config["body"], json!(child));
        let selected = FlowDiscovery::resolve_with_builtin_catalog(
            &entry,
            &registry,
            BTreeMap::new(),
            BTreeSet::new(),
        )
        .unwrap();
        let memory = resolve_flows_with_registry(&parent, Some(root.path()), &registry).unwrap();
        for (result, from_file) in [(selected, true), (memory, false)] {
            assert_eq!(result.definition, file.definition);
            assert_eq!(result.children, file.children);
            let mut sources = result.sources;
            assert_eq!(sources.contains(&entry), from_file);
            sources.insert(entry.clone());
            assert_eq!(sources, file.sources);
        }
        for (discovery, from_file) in [
            (
                discover_flows_with_registry(&parent, Some(root.path()), &registry).unwrap(),
                false,
            ),
            (
                FlowDiscovery::load_with_registry(&entry, &registry).unwrap(),
                true,
            ),
        ] {
            assert!(discovery.pending.is_empty());
            assert_eq!(discovery.definition, file.definition);
            assert_eq!(
                discovery.children[&"owner".into()]["body"].definition,
                file.children[&"owner".into()]["body"].definition
            );
            let serialized = serde_json::to_value(&discovery).unwrap();
            assert!(serialized.get("contracts").is_none());
            assert!(serialized.get("parameter_types").is_none());
            assert_eq!(
                serialized["children"]["owner"]["body"]["definition"],
                json!(child)
            );
            let mut sources = discovery.sources;
            assert_eq!(sources.contains(&entry), from_file);
            sources.insert(entry.clone());
            assert_eq!(sources, file.sources);
        }
        let embedded = load_embedded_workflow(&entry).unwrap();
        assert_eq!(embedded.definition.dependencies, parent.dependencies);
        assert_eq!(embedded.sources, file.sources);
        assert_eq!(embedded.children, file.children);
        let bundle = embedded_archive(
            "nested/root.json",
            &[("nested/root.json", parent), ("nested/child.json", child)],
        );
        // The captured build handoff replays the host result after source removal.
        fs::remove_file(root.path().join("child.json")).unwrap();
        fs::remove_file(&entry).unwrap();
        let replay = bundle.resolve(&registry).unwrap();
        assert_eq!(replay.definition, embedded.definition);
        assert_eq!(
            replay.children[&"owner".into()]["body"].definition,
            file.children[&"owner".into()]["body"].definition
        );
        assert_eq!(
            replay.sources,
            BTreeSet::from([
                PathBuf::from("nested/root.json"),
                PathBuf::from("nested/child.json")
            ])
        );
    }

    #[test]
    fn embedded_sources_resolve_nested_lexical_paths_with_host_contracts() {
        let leaf = definition(json!([]));
        let mut child = definition(json!([{
            "id":"nested", "kind":"test.source-api-owner", "config":{
                "body":{"path":"../leaf.json"},
                "ordinary":{"path":"../../outside.json"}
            }
        }]));
        child.dependencies.insert(
            "provider".into(),
            NodeDependency {
                path: Some("not-on-the-host-filesystem".into()),
                ..dependency("host-provider")
            },
        );
        let parent = definition(json!([{
            "id":"owner", "kind":"test.source-api-owner", "config":{
                "body":{"path":"nested/./child.json"},
                "ordinary":{"path":"never-read.json"}
            }
        }, {
            "id":"opaque", "kind":"unregistered.owner", "config":{
                "body":{"path":"never-read.json"}
            }
        }]));
        let archive = embedded_archive(
            "./flows/root.json",
            &[
                ("flows/root.json", parent.clone()),
                ("flows/nested/child.json", child.clone()),
                ("flows/unused/../leaf.json", leaf.clone()),
            ],
        );
        let archive =
            FlowSourceBundle::from_json(&serde_json::to_string(&archive).unwrap()).unwrap();
        let resolved = archive
            .resolve(&NodeRegistry::from_inventory().unwrap())
            .unwrap();
        assert_eq!(
            resolved.sources,
            BTreeSet::from([
                "flows/root.json".into(),
                "flows/nested/child.json".into(),
                "flows/leaf.json".into(),
            ])
        );
        assert_eq!(resolved.definition.dependencies, parent.dependencies);
        assert_eq!(
            resolved.definition.nodes[0].config["body"]["dependencies"],
            json!(child.dependencies)
        );
        assert_eq!(
            resolved.definition.nodes[0].config["body"]["nodes"][0]["config"]["body"],
            json!(leaf)
        );
        assert_eq!(
            resolved.definition.nodes[0].config["body"]["nodes"][0]["config"]["ordinary"],
            child.nodes[0].config["ordinary"]
        );
        assert_eq!(
            resolved.definition.nodes[0].config["ordinary"],
            parent.nodes[0].config["ordinary"]
        );
        assert_eq!(resolved.definition.nodes[1], parent.nodes[1]);
        let child_scope = &resolved.children[&"owner".into()]["body"];
        assert_eq!(
            child_scope.sources,
            BTreeSet::from(["flows/nested/child.json".into()])
        );
        let leaf_scope = &child_scope.children[&"nested".into()]["body"];
        assert_eq!(leaf_scope.definition, leaf);
        assert_eq!(
            leaf_scope.sources,
            BTreeSet::from(["flows/leaf.json".into()])
        );
        assert!(leaf_scope.children.is_empty());
        assert_eq!(resolved.children.len(), 1);
        assert_eq!(child_scope.children.len(), 1);
    }

    #[test]
    fn embedded_archive_serializes_native_paths_as_portable_slash_identities() {
        let archive = FlowSourceBundle {
            entry: PathBuf::from("flows").join("root.json"),
            sources: BTreeMap::from([
                (
                    PathBuf::from("flows").join("root.json"),
                    "root source".into(),
                ),
                (
                    PathBuf::from("flows").join("nested").join("child.json"),
                    "child source".into(),
                ),
            ]),
        };
        let value = serde_json::to_value(&archive).unwrap();
        assert_eq!(
            value,
            json!({
                "entry":"flows/root.json",
                "sources":{
                    "flows/root.json":"root source",
                    "flows/nested/child.json":"child source"
                }
            })
        );
        let restored = FlowSourceBundle::from_json(&value.to_string()).unwrap();
        assert_eq!(restored.entry, archive.entry);
        assert_eq!(restored.sources, archive.sources);
    }

    #[cfg(unix)]
    #[test]
    fn embedded_archive_serialization_rejects_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;

        let invalid = PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]));
        for archive in [
            FlowSourceBundle {
                entry: invalid.clone(),
                sources: BTreeMap::new(),
            },
            FlowSourceBundle {
                entry: "root.json".into(),
                sources: BTreeMap::from([(invalid, "source".into())]),
            },
        ] {
            assert!(
                serde_json::to_string(&archive)
                    .unwrap_err()
                    .to_string()
                    .contains("valid UTF-8")
            );
        }
    }

    fn unauthorized_archive_builtin() -> Result<WorkflowDefinition, crate::DefinitionParseError> {
        panic!("an unselected Call must not invoke a builtin definition callback")
    }

    fn archive_builtin_with_local_path() -> Result<WorkflowDefinition, crate::DefinitionParseError>
    {
        Ok(definition(json!([{
            "id":"nested", "kind":"test.source-api-owner", "config":{"body":{"path":"child.json"}}
        }])))
    }

    inventory::submit! {
        mf_runtime::BuiltinFlowRegistration::new("source-archive-test", "unauthorized", unauthorized_archive_builtin)
    }

    inventory::submit! {
        mf_runtime::BuiltinFlowRegistration::new("source-archive-test", "local-path", archive_builtin_with_local_path)
    }

    #[test]
    fn embedded_named_calls_require_selected_contract_before_reading_or_invoking_sources() {
        for source in [
            FlowSource::Path {
                path: "invalid.json".into(),
            },
            FlowSource::Builtin {
                builtin: "source-archive-test.unauthorized".into(),
            },
        ] {
            let parent = call_definition(json!(source));
            let mut archive = embedded_archive("root.json", &[("root.json", parent)]);
            archive
                .sources
                .insert("invalid.json".into(), "{not valid JSON}".into());
            let error = archive.resolve(&NodeRegistry::default()).err().unwrap();
            assert!(
                matches!(source_cause(&error), FlowResolutionError::Invalid { reference, message }
                if reference == "call" && message == "no selected provider metadata authorizes the Call `body` Flow parameter"),
                "{error}"
            );
        }
    }

    #[test]
    fn embedded_declared_parameters_allow_absent_null_and_present_values() {
        let child = definition(json!([]));
        let parent = definition(json!([
            {"id":"absent", "kind":"test.source-api-owner", "config":{}},
            {"id":"null", "kind":"test.source-api-owner", "config":{"body":null}},
            {"id":"present", "kind":"test.source-api-owner", "config":{"body":{"path":"child.json"}}}
        ]));
        let resolved = embedded_archive(
            "root.json",
            &[("root.json", parent), ("child.json", child.clone())],
        )
        .resolve(&NodeRegistry::from_inventory().unwrap())
        .unwrap();
        assert_eq!(resolved.definition.nodes[0].config, json!({}));
        assert_eq!(resolved.definition.nodes[1].config, json!({"body":null}));
        assert_eq!(resolved.definition.nodes[2].config["body"], json!(child));
        assert_eq!(
            resolved.sources,
            BTreeSet::from(["root.json".into(), "child.json".into()])
        );
    }

    #[test]
    fn embedded_unknown_builtins_fail_and_builtin_local_paths_have_no_archive_directory() {
        let registry = NodeRegistry::from_inventory().unwrap();
        for (builtin, reference, message) in [
            (
                "source-archive-test.unknown",
                "source-archive-test.unknown",
                "unknown built-in Flow",
            ),
            (
                "source-archive-test.local-path",
                "nested.config.body",
                "local Flow references require",
            ),
        ] {
            let parent = source_definition("test.source-api-owner", json!({"builtin":builtin}));
            // The path is present in the archive, but builtins cannot inherit its
            // entry's directory or otherwise acquire local source provenance.
            let archive = embedded_archive(
                "root.json",
                &[("root.json", parent), ("child.json", definition(json!([])))],
            );
            let error = archive.resolve(&registry).err().unwrap();
            assert!(
                matches!(source_cause(&error), FlowResolutionError::Invalid { reference: actual_reference, message: actual_message }
                if actual_reference == reference && actual_message.contains(message)),
                "{error}"
            );
        }
    }

    #[test]
    fn embedded_only_selected_fields_read_archive_sources_without_filesystem_fallback() {
        // This path exists relative to the process cwd, but is absent from the archive.
        let parent = source_definition("test.source-api-owner", json!({"path":"Cargo.toml"}));
        assert!(Path::new("Cargo.toml").is_file());
        let archive = embedded_archive("root.json", &[("root.json", parent.clone())]);
        let opaque = archive.resolve(&NodeRegistry::default()).unwrap();
        assert_eq!(opaque.definition, parent);
        let error = archive
            .resolve(&NodeRegistry::from_inventory().unwrap())
            .err()
            .unwrap();
        assert!(
            matches!(source_cause(&error), FlowResolutionError::Read { path, source }
            if path == Path::new("Cargo.toml") && source.kind() == io::ErrorKind::NotFound)
        );
        let archive = FlowSourceBundle {
            entry: "Cargo.toml".into(),
            sources: BTreeMap::new(),
        };
        let error = archive.resolve(&NodeRegistry::default()).err().unwrap();
        assert!(matches!(error, FlowResolutionError::Read { source, .. }
            if source.kind() == io::ErrorKind::NotFound));
    }

    #[test]
    fn embedded_paths_reject_escapes_absolute_names_and_normalized_collisions() {
        for path in [
            "../outside.json",
            "nested/../../outside.json",
            "/absolute.json",
            ".",
            "nested/..",
        ] {
            assert!(matches!(
                normalize_embedded_path(Path::new(path)),
                Err(FlowResolutionError::Invalid { .. })
            ));
            let archive = embedded_archive(path, &[]);
            assert!(matches!(
                archive.resolve(&NodeRegistry::default()),
                Err(FlowResolutionError::Invalid { .. })
            ));
            let archive = embedded_archive("root.json", &[(path, definition(json!([])))]);
            assert!(matches!(
                archive.resolve(&NodeRegistry::default()),
                Err(FlowResolutionError::Invalid { .. })
            ));
        }
        let collision = embedded_archive(
            "root.json",
            &[
                ("root.json", definition(json!([]))),
                ("nested/../root.json", definition(json!([]))),
            ],
        );
        assert!(
            matches!(collision.resolve(&NodeRegistry::default()), Err(FlowResolutionError::Invalid { message, .. })
            if message.contains("duplicate normalized"))
        );
        for path in ["../outside.json", "/absolute.json"] {
            let parent = source_definition("test.source-api-owner", json!({"path":path}));
            let error = embedded_archive("root.json", &[("root.json", parent)])
                .resolve(&NodeRegistry::from_inventory().unwrap())
                .err()
                .unwrap();
            assert!(matches!(
                source_cause(&error),
                FlowResolutionError::Invalid { .. }
            ));
        }
    }

    #[test]
    fn embedded_entry_establishes_normalized_cycle_identity() {
        let parent = source_definition(
            "test.source-api-owner",
            json!({"path":"nested/../root.json"}),
        );
        let error = embedded_archive("./root.json", &[("root.json", parent)])
            .resolve(&NodeRegistry::from_inventory().unwrap())
            .err()
            .unwrap();
        assert!(
            matches!(source_cause(&error), FlowResolutionError::Invalid { reference, message }
            if reference == "root.json" && message.contains("recursive Flow reference"))
        );
    }

    #[test]
    fn embedded_sources_preserve_typed_parse_errors_and_strict_archive_schema() {
        assert!(
            FlowSourceBundle::from_json(r#"{"entry":"root.json","sources":{},"contracts":{}}"#)
                .is_err()
        );
        let registry = NodeRegistry::from_inventory().unwrap();
        let mut archive = embedded_archive(
            "root.json",
            &[(
                "root.json",
                source_definition("test.source-api-owner", json!({"path":"invalid.json"})),
            )],
        );
        archive
            .sources
            .insert("invalid.json".into(), "{not valid JSON}".into());
        let error = archive.resolve(&registry).err().unwrap();
        assert!(
            matches!(source_cause(&error), FlowResolutionError::Parse { path, source: crate::DefinitionParseError::JsonParse { .. } }
            if path == Path::new("invalid.json"))
        );
        archive.entry = "invalid.json".into();
        let error = archive.resolve(&registry).err().unwrap();
        assert!(
            matches!(error, FlowResolutionError::Parse { path, source: crate::DefinitionParseError::JsonParse { .. } }
            if path == Path::new("invalid.json"))
        );
    }

    #[test]
    fn embedded_sources_retain_call_depth_and_combined_scope_limits() {
        for (kind, limit, diagnostic) in [
            (
                "test.source-api-owner",
                MAX_CALL_DEPTH,
                "nesting exceeds sixteen",
            ),
            (
                mf_runtime::LOOP_KIND,
                mf_telemetry::MAX_SCOPE_DEPTH,
                "exceeds twenty scopes",
            ),
        ] {
            let mut archive = FlowSourceBundle {
                entry: "0.json".into(),
                sources: BTreeMap::new(),
            };
            for index in 0..=limit + 1 {
                let value = if index == limit + 1 {
                    definition(json!([]))
                } else {
                    definition(json!([{
                        "id":"owner", "kind":kind, "config":{"body":{"path":format!("{}.json", index + 1)}}
                    }]))
                };
                archive.sources.insert(
                    format!("{index}.json").into(),
                    serde_json::to_string(&value).unwrap(),
                );
            }
            let mut resolver = Resolver {
                host_dependencies: true,
                embedded_sources: Some(archive.sources),
                extractors: BTreeMap::from([(kind.into(), body_parameter as FlowExtractor)]),
                ..Default::default()
            };
            let error = resolver
                .file(&archive.entry, &BTreeMap::new(), 0)
                .unwrap_err();
            assert_eq!(resolver.scope_depth, 0);
            assert!(resolver.stack.is_empty());
            assert!(
                matches!(source_cause(&error), FlowResolutionError::Invalid { message, .. }
                if message.contains(diagnostic)),
                "{error}"
            );
        }
    }

    #[test]
    fn embedded_sources_use_host_builtin_inventory() {
        let parent = source_definition(
            "test.source-api-owner",
            json!({"builtin":"source-catalog-test.example"}),
        );
        let resolved = embedded_archive("root.json", &[("root.json", parent)])
            .resolve(&NodeRegistry::from_inventory().unwrap())
            .unwrap();
        assert_eq!(
            resolved.definition.nodes[0].config["body"]["nodes"][0]["id"],
            "selected"
        );
        assert_eq!(
            resolved.sources,
            BTreeSet::from([PathBuf::from("root.json")])
        );
    }

    #[test]
    fn public_file_discovery_preserves_read_and_parse_error_sources() {
        let root = tempfile::tempdir().unwrap();
        let registry = NodeRegistry::default();
        let missing = root.path().join("missing.json");
        let error = FlowDiscovery::load_with_registry(&missing, &registry)
            .err()
            .unwrap();
        assert!(matches!(error, FlowResolutionError::Read { path, source }
            if path == missing && source.kind() == io::ErrorKind::NotFound));
        let invalid = root.path().join("invalid.json");
        fs::write(&invalid, "{not valid JSON}").unwrap();
        let error = FlowDiscovery::load_with_registry(&invalid, &registry)
            .err()
            .unwrap();
        assert!(
            matches!(error, FlowResolutionError::Parse { path, source:crate::DefinitionParseError::JsonParse { .. } }
            if path == fs::canonicalize(&invalid).unwrap())
        );
    }

    #[test]
    fn blank_parameter_names_fail_before_resolving_declared_sources() {
        for extractor in [
            (|config: &serde_json::Value| BTreeMap::from([("".into(), config["body"].clone())]))
                as FlowExtractor,
            (|config: &serde_json::Value| BTreeMap::from([("   ".into(), config["body"].clone())]))
                as FlowExtractor,
            (|config: &serde_json::Value| {
                BTreeMap::from([("\u{2000}".into(), config["body"].clone())])
            }) as FlowExtractor,
        ] {
            let mut parent =
                source_definition("test.flow-source-owner", json!({"path":"never-read.json"}));
            let mut resolver = source_resolver();
            resolver
                .extractors
                .insert("test.flow-source-owner".into(), extractor);
            let error = resolver.resolve(&mut parent, None).unwrap_err();
            assert!(
                matches!(error, FlowResolutionError::Invalid { reference, message }
                if reference == "owner" && message.contains("blank configuration parameter declaration"))
            );
            assert!(resolver.sources.is_empty());
        }
    }

    #[test]
    fn invalid_call_selectors_and_parameter_shapes_have_typed_configuration_errors() {
        for (flow, expected_reference, expected_message) in [
            ("   ", "call", "Flow alias must not be blank"),
            ("missing", "missing", "unknown Flow alias"),
        ] {
            let mut parent = call_definition(json!({"path":"child/a.json"}));
            parent.nodes[0].config["flow"] = json!(flow);
            let error = source_resolver().resolve(&mut parent, None).unwrap_err();
            assert!(
                matches!(error, FlowResolutionError::Invalid { reference, message }
                if reference == expected_reference && message == expected_message)
            );
        }
        for (mut parent, reference) in [
            (
                definition(json!([{"id":"call", "kind":CALL_KIND, "config":{"flow":42}}])),
                "call",
            ),
            (
                source_definition("test.flow-source-owner", json!({"path":42})),
                "owner.config.body",
            ),
        ] {
            let error = source_resolver().resolve(&mut parent, None).unwrap_err();
            assert!(
                matches!(error, FlowResolutionError::Configuration { node, source }
                if node == reference && source.is_data())
            );
        }
    }

    #[test]
    fn source_resolution_requires_lexical_directories() {
        let mut parent = source_definition(
            "test.flow-source-owner",
            json!({"path":"not-in-the-process-cwd.json"}),
        );
        let error = source_resolver().resolve(&mut parent, None).unwrap_err();
        assert!(
            matches!(error, FlowResolutionError::Invalid { reference, message }
            if reference == "owner.config.body" && message.contains("source directory"))
        );
    }

    fn invalid_catalog_definition() -> Result<WorkflowDefinition, crate::DefinitionParseError> {
        WorkflowDefinition::from_json("{not valid JSON}")
    }

    inventory::submit! {
        mf_runtime::BuiltinFlowRegistration::new("source-catalog-test", "invalid", invalid_catalog_definition)
    }

    #[cfg(feature = "codegen")]
    #[test]
    fn invalid_requested_catalog_definition_preserves_its_named_parse_error() {
        let mut catalog = BTreeMap::new();
        let error = snapshot_pending_builtins(
            &BTreeSet::from(["builtin:source-catalog-test.invalid".into()]),
            &BTreeSet::new(),
            &mut catalog,
        )
        .unwrap_err();
        assert!(
            matches!(error, FlowResolutionError::Parse { path, source:crate::DefinitionParseError::JsonParse { .. } }
            if path == Path::new("builtin:source-catalog-test.invalid"))
        );
        assert!(catalog.is_empty());
    }

    #[test]
    fn declared_fields_freeze_with_lexical_paths_and_collect_dependencies() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("child/provider")).unwrap();
        let grandchild = definition(json!([]));
        write_definition(root.path(), "child/grandchild.json", &grandchild);
        let mut child = definition(json!([{
            "id": "nested", "kind": "test.flow-source-owner",
            "config": {"body": {"path": "grandchild.json"}}
        }]));
        child.dependencies.insert(
            "provider".into(),
            NodeDependency {
                path: Some("provider".into()),
                features: vec!["flow".into()],
                ..dependency("lexical-provider")
            },
        );
        write_definition(root.path(), "child/flow.json", &child);
        let mut parent = definition(json!([{
            "id": "owner", "kind": "test.flow-source-owner",
            "config": {"body": {"path": "child/flow.json"}, "data": {"path": "not-a-flow.json"}}
        }]));
        let mut resolver = source_resolver();
        resolver.resolve(&mut parent, Some(root.path())).unwrap();
        resolver.finish(&mut parent);
        assert_eq!(resolver.sources.len(), 2);
        assert!(
            parent.nodes[0].config["body"]["nodes"][0]["config"]["body"]
                .get("nodes")
                .is_some()
        );
        assert_eq!(
            parent.nodes[0].config["data"],
            json!({"path": "not-a-flow.json"})
        );
        let dependency = parent.dependencies.values().next().unwrap();
        assert_eq!(
            dependency.path.as_ref().unwrap(),
            &fs::canonicalize(root.path().join("child/provider")).unwrap()
        );
    }

    #[test]
    fn ordinary_json_remains_opaque() {
        let child = definition(json!([]));
        let mut parent = definition(json!([{
            "id": "owner", "kind": "test.ordinary", "config": {
                "body": child, "other":child,
                "path":{"path":"never-read.json"}, "builtin":{"builtin":"not.linked"}
            }
        }]));
        let original = parent.clone();
        source_resolver().resolve(&mut parent, None).unwrap();
        assert_eq!(parent, original);
        let plan = source_resolver().plan(&parent).unwrap();
        assert!(plan.graph.children.is_empty());
        assert!(
            crate::describe_compiled(&plan)
                .unwrap()
                .loop_bodies
                .is_empty()
        );
    }

    #[test]
    fn declared_field_accepts_inline_and_direct_definitions() {
        for version in [
            mf_runtime::WorkflowDefinitionVersion::V2026_09_26,
            mf_runtime::WorkflowDefinitionVersion::V2026_10_28,
        ] {
            let child = definition(json!([]));
            for value in [json!({"definition":child}), json!(child)] {
                let mut parent = source_definition("test.flow-source-owner", value);
                parent.version = version;
                source_resolver().resolve(&mut parent, None).unwrap();
                assert_eq!(parent.nodes[0].config["body"], json!(child));
                let plan = source_resolver().plan(&parent).unwrap();
                assert_eq!(plan.graph.children.len(), 1);
                assert_eq!(
                    crate::describe_compiled(&plan).unwrap().loop_bodies.len(),
                    1
                );
            }
        }
    }

    #[test]
    fn unlinked_frozen_fields_have_no_source_or_dependency_authority() {
        let root = tempfile::tempdir().unwrap();
        let mut child = definition(json!([{
            "id":"nested", "kind":"unlinked.nested", "config":{"body":{"path":"never-read.json"}}
        }]));
        child.dependencies.insert(
            "untrusted".into(),
            NodeDependency {
                path: Some("nonexistent-provider".into()),
                ..dependency("untrusted-provider")
            },
        );
        let parent = source_definition("unlinked.owner", json!(child));
        let original = parent.nodes[0].config.clone();
        let discovery = discover_unlinked_flows(&parent, Some(root.path())).unwrap();
        assert_eq!(
            discovery.pending,
            BTreeSet::from(["node:unlinked.owner".into()])
        );
        assert!(discovery.sources.is_empty());
        assert!(discovery.definition.dependencies.is_empty());
        assert_eq!(discovery.definition.nodes[0].config, original);
    }

    #[test]
    fn existing_versions_discover_unknown_receivers_but_do_not_read_ordinary_paths() {
        for version in [
            mf_runtime::WorkflowDefinitionVersion::V2026_09_26,
            mf_runtime::WorkflowDefinitionVersion::V2026_10_28,
        ] {
            let mut parent =
                source_definition("test.flow-source-owner", json!({"path":"missing.json"}));
            parent.version = version;
            let original = parent.clone();
            for discovery in [
                discover_unlinked_flows(&parent, None).unwrap(),
                discover_flows_with_registry(&parent, None, &NodeRegistry::default()).unwrap(),
            ] {
                assert_eq!(
                    discovery.pending,
                    BTreeSet::from(["node:test.flow-source-owner".into()])
                );
                assert_eq!(discovery.definition, original);
                assert!(discovery.sources.is_empty());
            }
            assert!(source_resolver().resolve(&mut parent, None).is_err());
            parent.nodes[0].kind = "test.ordinary".into();
            source_resolver().resolve(&mut parent, None).unwrap();
        }
    }

    #[test]
    fn declared_roles_do_not_make_optional_parameters_required() {
        let mut parent = definition(json!([
            {"id":"absent", "kind":"test.flow-source-owner", "config":{}},
            {"id":"null", "kind":"test.flow-source-owner", "config":{"body":null}}
        ]));
        source_resolver().resolve(&mut parent, None).unwrap();
        assert_eq!(parent.nodes[0].config, json!({}));
        assert_eq!(parent.nodes[1].config, json!({"body": null}));
    }

    #[test]
    fn unlinked_named_call_with_empty_child_retains_its_pending_receiving_contract() {
        let root = tempfile::tempdir().unwrap();
        let child = definition(json!([]));
        write_definition(root.path(), "empty.json", &child);
        let core = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("builtin-nodes/core");
        let mut parent = call_definition(json!({"path":"empty.json"}));
        parent.dependencies = serde_json::from_value(json!({
            "core":{"package":"mfn-core", "path":core}
        }))
        .unwrap();
        let discovery = discover_unlinked_flows(&parent, Some(root.path())).unwrap();
        assert_eq!(
            discovery.pending,
            BTreeSet::from([format!("node:{CALL_KIND}")])
        );
        assert!(discovery.children.is_empty());
        assert_eq!(
            discovery.sources,
            BTreeSet::from([fs::canonicalize(root.path().join("empty.json")).unwrap()])
        );
        assert_eq!(discovery.definition.nodes[0].config["body"], json!(child));
        assert_eq!(discovery.definition.dependencies.len(), 1);
        assert_eq!(
            discovery.definition.dependencies["core"].package,
            "mfn-core"
        );
        assert_eq!(
            discovery.definition.dependencies["core"].path,
            Some(fs::canonicalize(core).unwrap())
        );
        assert!(discovery.definition.flows.is_empty());
    }

    #[test]
    fn selected_call_body_is_frozen_in_the_same_canonical_order_as_its_own_plan() {
        let child = definition(json!([
            {"id":"a-target", "kind":"test.ordinary", "inputs":{
                "input":{"from":{"node":"z-source", "port":"value"}}
            }},
            {"id":"z-source", "kind":"test.ordinary"}
        ]));
        let mut parent = definition(json!([{
            "id":"call", "kind":CALL_KIND, "config":{"flow":"explicit", "body":child}
        }]));
        let mut resolver = source_resolver();
        resolver.resolve(&mut parent, None).unwrap();
        let plan = source_resolver().plan(&parent).unwrap();
        let config: CallConfig =
            serde_json::from_value(plan.graph.definition.nodes[0].config.clone()).unwrap();
        let child_plan = source_resolver().plan(&child).unwrap();
        assert_eq!(*config.body.unwrap(), child_plan.graph.definition);
        assert_eq!(
            child_plan.graph.execution_order,
            vec!["z-source".into(), "a-target".into()]
        );
        assert_eq!(plan.graph.children.len(), 1);
    }

    fn call_codec_body() -> Result<WorkflowDefinition, crate::DefinitionParseError> {
        Ok(definition(json!([{
            "id":"apply", "kind":"test.flow-source-owner", "config":{"body":null}
        }])))
    }

    inventory::submit! {
        mf_runtime::BuiltinFlowRegistration::new("compiler-call-codec", "body", call_codec_body)
    }

    #[test]
    fn call_null_body_keeps_alias_selection_and_selector_validation_strict() {
        for null_body in [false, true] {
            let mut parent = call_definition(json!({"builtin":"compiler-call-codec.body"}));
            if null_body {
                parent.nodes[0].config["body"] = json!(null);
            }
            source_resolver().resolve(&mut parent, None).unwrap();
            let config: CallConfig =
                serde_json::from_value(parent.nodes[0].config.clone()).unwrap();
            assert_eq!(*config.body.unwrap(), call_codec_body().unwrap());
        }
        let body = definition(json!([{
            "id":"value", "kind":"builtin.constant", "config":{"value":1}
        }]));
        let mut parent = definition(json!([{
            "id":"call", "kind":CALL_KIND,
            "config":{"flow":"alias", "body":body, "unexpected":true}
        }]));
        // Resolution owns the selector, not the receiver's ordinary schema. It
        // must preserve unknown fields for the actual provider decoder to reject.
        source_resolver().resolve(&mut parent, None).unwrap();
        assert_eq!(parent.nodes[0].config["unexpected"], json!(true));
        assert_eq!(parent.nodes[0].config["body"], json!(body));
    }

    #[test]
    fn explicit_call_bodies_require_selected_receiving_authority_before_any_io() {
        let root = tempfile::tempdir().unwrap();
        let mut child = definition(json!([{
            "id":"nested", "kind":CALL_KIND, "config":{"flow":"missing"}
        }]));
        child.flows.insert(
            "missing".into(),
            FlowSource::Path {
                path: "must-not-read.json".into(),
            },
        );
        let mut poisoned_child = child.clone();
        poisoned_child.dependencies = serde_json::from_value(json!({"untrusted":{
            "package":"untrusted", "path":"must-not-canonicalize-provider"
        }}))
        .unwrap();
        let mut case_count = 0;
        for (body_case, body) in [
            ("direct", json!(child)),
            ("wrapped", json!({"definition":child})),
            ("direct poison", json!(poisoned_child)),
            ("wrapped poison", json!({"definition":poisoned_child})),
            ("path", json!({"path":"must-not-read.json"})),
            ("builtin", json!({"builtin":"compiler-call-codec.body"})),
        ] {
            let parent = definition(json!([{
                "id":"explicit", "kind":CALL_KIND, "config":{"flow":"frozen", "body":body}
            }]));
            // Plain and zero-parameter Subgraph providers both extract no children.
            for extractors in [
                BTreeMap::new(),
                BTreeMap::from([(CALL_KIND.into(), no_parameters as FlowExtractor)]),
            ] {
                for discovery in [false, true] {
                    let case = format!(
                        "{body_case}, selected={}, discovery={discovery}",
                        !extractors.is_empty()
                    );
                    let mut resolver = Resolver {
                        extractors: extractors.clone(),
                        discovery,
                        ..Default::default()
                    };
                    let mut candidate = parent.clone();
                    case_count += 1;
                    let result = resolver.resolve(&mut candidate, Some(root.path()));
                    assert_eq!(candidate.nodes[0].config, parent.nodes[0].config, "{case}");
                    if discovery && extractors.is_empty() {
                        assert!(result.is_ok(), "{case}: {result:?}");
                        assert_eq!(
                            resolver.pending,
                            BTreeSet::from([format!("node:{CALL_KIND}")]),
                            "{case}"
                        );
                    } else {
                        assert!(
                            matches!(result, Err(FlowResolutionError::Invalid { .. })),
                            "{case}: {result:?}"
                        );
                    }
                    assert!(resolver.sources.is_empty(), "{case}");
                    assert!(resolver.dependencies.is_empty(), "{case}");
                }
            }
        }
        assert_eq!(case_count, 24);
    }

    #[test]
    fn ordinary_call_body_types_do_not_authorize_named_or_explicit_sources() {
        for extractor in [no_parameters as FlowExtractor] {
            for explicit in [false, true] {
                let mut parent =
                    call_definition(json!({"builtin":"source-archive-test.unauthorized"}));
                if explicit {
                    parent.nodes[0].config["body"] = json!({"path":"must-not-read.json"});
                }
                let config = parent.nodes[0].config.clone();
                let mut receiver = Resolver {
                    extractors: BTreeMap::from([(CALL_KIND.into(), extractor)]),
                    ..Default::default()
                };
                let error = receiver.resolve(&mut parent, None).unwrap_err();
                assert!(
                    matches!(error, FlowResolutionError::Invalid { reference, message }
                    if reference == "call" && message == "selected Call provider does not declare the `body` Flow parameter")
                );
                assert_eq!(parent.nodes[0].config, config);
                assert!(receiver.sources.is_empty());
                assert!(receiver.dependencies.is_empty());
            }
        }
    }

    #[test]
    fn external_loop_sources_stop_at_the_existing_combined_scope_limit_before_poison_io() {
        let root = tempfile::tempdir().unwrap();
        let loop_source = |next: usize| {
            definition(json!([{
                "id":"repeat", "kind":mf_runtime::LOOP_KIND, "config":{
                    "max_iterations":1, "variables":[{"name":"count","type":"int"}],
                    "body":{"path":format!("body-{next}.json")}
                }
            }]))
        };
        for index in 0..mf_telemetry::MAX_SCOPE_DEPTH {
            write_definition(
                root.path(),
                &format!("body-{index}.json"),
                &loop_source(index + 1),
            );
        }
        let poison = root
            .path()
            .join(format!("body-{}.json", mf_telemetry::MAX_SCOPE_DEPTH));
        fs::write(&poison, "must not be read or parsed as a workflow").unwrap();
        let mut parent = loop_source(0);
        let mut receiver = source_resolver();
        receiver
            .extractors
            .insert(mf_runtime::LOOP_KIND.into(), body_parameter);
        let error = receiver
            .resolve(&mut parent, Some(root.path()))
            .unwrap_err();
        assert!(
            matches!(source_cause(&error), FlowResolutionError::Invalid { reference, message }
            if reference == "repeat.config.body"
                && message == "combined container nesting exceeds twenty scopes"),
            "{error}"
        );
        assert_eq!(
            receiver.sources,
            (0..mf_telemetry::MAX_SCOPE_DEPTH)
                .map(
                    |index| fs::canonicalize(root.path().join(format!("body-{index}.json")))
                        .unwrap()
                )
                .collect()
        );
        assert!(
            !receiver
                .sources
                .contains(&fs::canonicalize(poison).unwrap())
        );
        assert_eq!(receiver.scope_depth, 0);
        assert!(receiver.stack.is_empty());
        // Failed descent must return the resource budget to its original value.
        assert!(
            receiver
                .parameter(
                    json!(definition(json!([]))),
                    None,
                    &BTreeMap::new(),
                    0,
                    "replay"
                )
                .unwrap()
                .is_some()
        );
        assert_eq!(receiver.scope_depth, 0);
    }

    #[test]
    fn source_scope_budget_counts_call_sources_once() {
        let root = tempfile::tempdir().unwrap();
        write_definition(root.path(), "empty.json", &definition(json!([])));
        for selected in [false, true] {
            let mut parent = call_definition(json!({"path":"empty.json"}));
            let mut receiver = if selected {
                source_resolver()
            } else {
                Resolver {
                    discovery: true,
                    unlinked: true,
                    ..Default::default()
                }
            };
            receiver.scope_depth = mf_telemetry::MAX_SCOPE_DEPTH - 1;
            receiver.resolve(&mut parent, Some(root.path())).unwrap();
            assert_eq!(
                receiver.sources,
                BTreeSet::from([fs::canonicalize(root.path().join("empty.json")).unwrap()])
            );
            assert_eq!(receiver.scope_depth, mf_telemetry::MAX_SCOPE_DEPTH - 1);
            // The same source, at the limit rather than one below it, must not
            // access even a nonexistent file through either alias entry point.
            let mut over = call_definition(json!({"path":"must-not-read.json"}));
            receiver.scope_depth = mf_telemetry::MAX_SCOPE_DEPTH;
            let error = receiver.resolve(&mut over, Some(root.path())).unwrap_err();
            assert!(
                matches!(error, FlowResolutionError::Invalid { ref message, .. }
                if message == "combined container nesting exceeds twenty scopes")
            );
            assert_eq!(receiver.scope_depth, mf_telemetry::MAX_SCOPE_DEPTH);
        }
    }

    #[test]
    fn loop_body_does_not_consume_the_named_call_depth_budget() {
        let leaf = definition(json!([{
            "id":"repeat", "kind":mf_runtime::LOOP_KIND,
            "config":{"max_iterations":1,"variables":[{"name":"count","type":"int"}],
                "body":{"version":"2026-10-28","dependencies":{},
                    "nodes":[{"id":"user","kind":"test.ordinary"}]}}
        }]));
        for calls in [16, 17] {
            let mut parent = leaf.clone();
            for index in 0..calls {
                parent = definition(json!([{
                    "id":format!("call-{index}"), "kind":CALL_KIND,
                    "config":{"flow":"explicit", "body":parent}
                }]));
            }
            let mut resolver = source_resolver();
            resolver
                .extractors
                .insert(mf_runtime::LOOP_KIND.into(), body_parameter);
            let result = resolver.resolve(&mut parent, None);
            if calls == 16 {
                let resolved = resolver.into_resolved(result.unwrap());
                let plan = crate::compiler::plan_resolved_workflow(&resolved).unwrap();
                let mut graph = &plan.graph;
                for index in (0..calls).rev() {
                    assert_eq!(graph.children.len(), 1);
                    graph = &graph.children[&format!("call-{index}").into()]["body"];
                }
                let body = &graph.children[&"repeat".into()]["body"];
                assert_eq!(
                    body.execution_order
                        .iter()
                        .map(DefinitionId::as_str)
                        .collect::<Vec<_>>(),
                    ["%loop", "user"]
                );
                assert!(body.children.is_empty());
                let replay = crate::compiler::plan_resolved_workflow(&resolved).unwrap();
                assert_eq!(replay.graph, plan.graph);
            } else {
                assert!(
                    matches!(result, Err(FlowResolutionError::Invalid { ref message, .. })
                    if message == "Flow nesting exceeds sixteen calls")
                );
            }
        }
    }

    #[test]
    fn iteration_boundary_validation_precedes_nested_config_resolution() {
        let mut parent = definition(json!([{
            "id":"outer", "kind":"builtin.iteration", "config":{
                "body":{
                    "version":"2026-10-28", "dependencies":{},
                    "nodes":[{"id":"nested", "kind":"builtin.iteration"}],
                    "outputs":[{"name":"result", "node":"nested", "port":"results"}]
                }
            }
        }]));
        let mut resolver = source_resolver();
        resolver
            .extractors
            .insert(mf_runtime::ITERATION_KIND.into(), body_parameter);
        let error = resolver.resolve(&mut parent, None).unwrap_err();
        assert!(matches!(
            error,
            FlowResolutionError::ChildStructure {
                source: crate::ScopeStructureError::NestedScope { .. },
                ..
            }
        ));
        assert!(error.to_string().contains("nested"));
        resolver
            .extractors
            .insert(mf_runtime::ITERATION_KIND.into(), no_parameters);
        let original = parent.clone();
        resolver.resolve(&mut parent, None).unwrap();
        assert_eq!(parent, original);
        assert!(resolver.sources.is_empty());
    }

    #[test]
    fn callback_values_authorize_actual_children_and_handoffs_reject_old_metadata() {
        let child = definition(json!([]));
        let mut parent = definition(json!([{
            "id":"owner", "kind":"test.flow-source-owner", "config":{
                "selected":{"definition":child},
                "body":{"path":"must-not-read.json"},
                "ordinary":{"version":"2026-10-28", "dependencies":{}, "nodes":[]}
            }
        }]));
        let mut resolver = source_resolver();
        resolver
            .extractors
            .insert("test.flow-source-owner".into(), |config| {
                BTreeMap::from([("body".into(), config["selected"].clone())])
            });
        let scope = resolver.resolve(&mut parent, None).unwrap();
        let resolved = resolver.into_resolved(scope);
        assert_eq!(resolved.children[&"owner".into()]["body"].definition, child);
        assert_eq!(resolved.definition.nodes[0].config["body"], json!(child));
        assert_eq!(resolved.children.len(), 1);
        assert_eq!(resolved.children[&"owner".into()].len(), 1);
        let value = serde_json::to_value(&resolved).unwrap();
        let replay: ResolvedWorkflow = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(replay, resolved);
        for field in ["contracts", "parameter_types"] {
            let mut old = value.clone();
            old[field] = json!({});
            assert!(serde_json::from_value::<ResolvedWorkflow>(old).is_err());
        }

        let discovery = resolve_using(
            Resolver {
                discovery: true,
                ..source_resolver()
            },
            &source_definition("test.flow-source-owner", json!(child)),
            None,
        )
        .unwrap();
        let mut value = serde_json::to_value(discovery).unwrap();
        value["contracts"] = json!({});
        assert!(serde_json::from_value::<FlowDiscovery>(value).is_err());
    }

    #[test]
    fn partial_discovery_keeps_pending_sources_out_of_the_actual_child_tree() {
        let child = definition(json!([{
            "id":"missing", "kind":"test.flow-source-owner", "config":{
                "body":{"builtin":"unavailable.scope"}
            }
        }]));
        let parent = definition(json!([
            {"id":"owner", "kind":"test.flow-source-owner", "config":{"body":child}},
            {"id":"sibling", "kind":"test.flow-source-owner", "config":{"body":definition(json!([]))}}
        ]));
        let discovery = resolve_using(
            Resolver {
                discovery: true,
                ..source_resolver()
            },
            &parent,
            None,
        )
        .unwrap();
        assert_eq!(
            discovery.pending,
            BTreeSet::from(["builtin:unavailable.scope".into()])
        );
        assert!(
            discovery.children[&"sibling".into()]["body"]
                .pending
                .is_empty()
        );
        let child = &discovery.children[&"owner".into()]["body"];
        assert_eq!(child.pending, discovery.pending);
        assert!(child.sources.is_empty());
        assert!(child.children.is_empty());
        assert_eq!(
            child.definition.nodes[0].config["body"],
            json!({"builtin":"unavailable.scope"})
        );
    }

    #[test]
    fn canonical_source_cycles_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let parent = definition(json!([{
            "id": "owner", "kind": "test.flow-source-owner", "config": {"body": {"path": "./flow.json"}}
        }]));
        let path = root.path().join("flow.json");
        write_definition(root.path(), "flow.json", &parent);
        let error = source_resolver()
            .file(&path, &BTreeMap::new(), 0)
            .err()
            .unwrap();
        assert!(error.to_string().contains("recursive Flow reference"));
    }
}
