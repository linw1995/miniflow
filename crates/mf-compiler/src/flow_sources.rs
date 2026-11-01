use crate::{NodeDependency, WorkflowDefinition};
use mf_runtime::{
    CALL_KIND, CallConfig, FlowSource, MAX_CALL_DEPTH, NodeDefinition, ResolvedFlowArgument,
    WorkflowDefinitionVersion,
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
    #[snafu(display("could not expand built-in Flow `{reference}`: {source}"))]
    Expansion {
        reference: String,
        source: mf_runtime::NodeBuildError,
    },
    #[snafu(display("could not describe injected Flow `{reference}`: {source}"))]
    Metadata {
        reference: String,
        #[snafu(source(from(crate::WorkflowCompileError, Box::new)))]
        source: Box<crate::WorkflowCompileError>,
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

const METADATA_EXPANSION_KIND: &str = "compiler.metadata_expansion";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MetadataExpansionRequest {
    builtin: String,
    config: serde_json::Value,
    arguments: BTreeMap<String, ResolvedFlowArgument>,
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

/// Finalize frozen requests without reinterpreting dependency ownership or source paths.
pub fn finalize_metadata(
    definition: &WorkflowDefinition,
    registry: &mf_runtime::NodeRegistry,
) -> Result<WorkflowDefinition, FlowResolutionError> {
    let mut resolver = Resolver {
        registry: Some(registry),
        host_dependencies: true,
        ..Default::default()
    };
    let mut definition = definition.clone();
    resolver.definition(&mut definition, None, &BTreeMap::new(), 0, "workflow")?;
    Ok(definition)
}

fn needs_resolution(
    definition: &WorkflowDefinition,
    registry: Option<&mf_runtime::NodeRegistry>,
) -> bool {
    definition
        .nodes
        .iter()
        .any(|node| node.kind == METADATA_EXPANSION_KIND)
        || !definition.flows.is_empty()
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
        if definition.nodes.len() == 1
            && definition.nodes[0].kind == METADATA_EXPANSION_KIND
            && let Some(registry) = self.registry
        {
            ensure!(
                depth <= MAX_CALL_DEPTH,
                InvalidSnafu {
                    reference,
                    message: "Flow nesting exceeds sixteen calls"
                }
            );
            let request: MetadataExpansionRequest = serde_json::from_value(
                definition.nodes[0].config.clone(),
            )
            .context(ConfigurationSnafu {
                node: "metadata_expansion",
            })?;
            let builtins = mf_runtime::BuiltinFlowRegistry::from_inventory()?;
            let registration = builtins.get(&request.builtin).context(InvalidSnafu {
                reference: &request.builtin,
                message: "unknown metadata-dependent built-in Flow",
            })?;
            let configure = registration.configure_metadata.context(InvalidSnafu {
                reference: &request.builtin,
                message: "built-in Flow no longer supports metadata expansion",
            })?;
            let mut described = BTreeMap::new();
            for (alias, argument) in request.arguments {
                let mut resolved = argument.definition;
                self.definition(&mut resolved, None, inherited, depth + 1, &alias)
                    .context(SourceSnafu {
                        path: PathBuf::from(format!("argument:{alias}")),
                    })?;
                // Retain authored interface declarations for provider policy checks.
                let frozen = resolved;
                let metadata = crate::compiler::describe_flow_argument(&alias, &frozen, registry)
                    .context(MetadataSnafu { reference: &alias })?;
                described.insert(
                    alias,
                    mf_runtime::DescribedFlowArgument {
                        definition: frozen,
                        description: argument.description,
                        parameters: argument.parameters,
                        metadata,
                    },
                );
            }
            let mut expanded = configure(request.config, described).context(ExpansionSnafu {
                reference: &request.builtin,
            })?;
            // Preserve dependency selections made by the source-resolution host.
            for dependency in expanded.dependencies.values_mut() {
                if let Some(selected) = definition
                    .dependencies
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
                &mut expanded,
                directory,
                inherited,
                depth + 1,
                &request.builtin,
            )?;
            *definition = expanded;
            return Ok(());
        }
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
        version: WorkflowDefinitionVersion,
        enclosing_flows: &BTreeMap<String, FlowSource>,
    ) -> Result<WorkflowDefinition, FlowResolutionError> {
        ensure!(
            depth <= MAX_CALL_DEPTH,
            InvalidSnafu {
                reference: "builtin",
                message: "Flow nesting exceeds sixteen calls"
            }
        );
        match source {
            FlowSource::Body { body } => {
                ensure!(
                    version.supports_flow_arguments(),
                    InvalidSnafu {
                        reference: "body",
                        message: "inline Flow bodies require version 2026-11-01"
                    }
                );
                // Dependencies are already canonicalized in the enclosing scope.
                // Do not run `definition` here: it would interpret inherited paths
                // again relative to this body's source directory.
                let mut definition = WorkflowDefinition {
                    version,
                    execution: None,
                    dependencies: available.clone(),
                    flows: BTreeMap::new(),
                    nodes: body.nodes.clone(),
                    edges: body.edges.clone(),
                    control_edges: body.control_edges.clone(),
                    order_edges: body.order_edges.clone(),
                    outputs: body.outputs.clone(),
                    clusters: body.clusters.clone(),
                };
                // Like loop/iteration bodies, inline Flows use lexical aliases.
                // File and provider definitions deliberately keep their own scope.
                self.nodes(
                    &mut definition.nodes,
                    enclosing_flows,
                    directory,
                    available,
                    depth,
                    version,
                )?;
                Ok(definition)
            }
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
                ensure!(
                    flows.is_empty() || version.supports_flow_arguments(),
                    InvalidSnafu {
                        reference: builtin,
                        message: "Flow arguments require version 2026-11-01"
                    }
                );
                let registry = mf_runtime::BuiltinFlowRegistry::from_inventory()?;
                let registration = registry.get(builtin).context(InvalidSnafu {
                    reference: builtin,
                    message: "unknown built-in Flow; link its provider before resolving sources",
                })?;
                // Resolve injections in the caller's directory BEFORE entering the provider.
                let mut injected = BTreeMap::new();
                for (alias, argument) in flows {
                    injected.insert(
                        alias.clone(),
                        ResolvedFlowArgument {
                            definition: self.source(
                                &argument.source,
                                directory,
                                available,
                                depth + 1,
                                version,
                                enclosing_flows,
                            )?,
                            description: argument.description.clone(),
                            parameters: argument.parameters.clone(),
                        },
                    );
                }
                let mut definition = if let Some(configure) = registration.configure_metadata {
                    if let Some(registry) = self.registry {
                        let mut described = BTreeMap::new();
                        for (alias, argument) in injected {
                            let mut definition = argument.definition;
                            self.definition(&mut definition, None, available, depth + 1, &alias)?;
                            let metadata = crate::compiler::describe_flow_argument(
                                &alias,
                                &definition,
                                registry,
                            )
                            .context(MetadataSnafu { reference: &alias })?;
                            described.insert(
                                alias,
                                mf_runtime::DescribedFlowArgument {
                                    definition,
                                    description: argument.description,
                                    parameters: argument.parameters,
                                    metadata,
                                },
                            );
                        }
                        configure(config.clone(), described)
                            .context(ExpansionSnafu { reference: builtin })?
                    } else {
                        // Preserve a source-free request until the selected providers are linked.
                        let request = MetadataExpansionRequest {
                            builtin: builtin.clone(),
                            config: config.clone(),
                            arguments: injected,
                        };
                        let mut definition = (registration.definition)().context(ParseSnafu {
                            path: PathBuf::from(format!("builtin:{builtin}")),
                        })?;
                        definition.nodes = vec![
                            serde_json::from_value(serde_json::json!({
                                "id": "metadata_expansion", "kind": METADATA_EXPANSION_KIND,
                                "config": request,
                            }))
                            .context(ConfigurationSnafu {
                                node: "metadata_expansion",
                            })?,
                        ];
                        definition.edges.clear();
                        definition.control_edges.clear();
                        definition.order_edges.clear();
                        definition.outputs.clear();
                        definition.clusters.clear();
                        definition
                    }
                } else if let Some(configure) = registration.configure {
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
                    Box::new(self.source(
                        source,
                        directory,
                        available,
                        depth + 1,
                        version,
                        flows,
                    )?)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn definition(flows: Value, nodes: Value) -> WorkflowDefinition {
        serde_json::from_value(json!({
            "version": "2026-11-01", "dependencies": {},
            "flows": flows, "nodes": nodes
        }))
        .unwrap()
    }

    fn call(alias: &str) -> Value {
        json!({"id": "call", "kind": CALL_KIND, "config": {"flow": alias}})
    }

    fn child(definition: &WorkflowDefinition) -> WorkflowDefinition {
        serde_json::from_value::<CallConfig>(definition.nodes[0].config.clone())
            .unwrap()
            .definition
            .map(|definition| *definition)
            .unwrap()
    }

    #[test]
    fn inline_calls_resolve_enclosing_aliases_and_inherit_canonical_dependencies() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(
            nested.join("leaf.json"),
            serde_json::to_string(&definition(json!({}), json!([]))).unwrap(),
        )
        .unwrap();
        let mut authored = definition(
            json!({
                "inline": {"body": {"nodes": [call("leaf")]}},
                "leaf": {"path": "nested/leaf.json"}
            }),
            json!([call("inline")]),
        );
        authored.dependencies.insert(
            "provider".into(),
            serde_json::from_value(json!({"package": "test-provider", "path": "."})).unwrap(),
        );
        let resolved = resolve_flows(&authored, Some(directory.path())).unwrap();
        let inline = child(&resolved.definition);
        assert_eq!(inline.version, authored.version);
        assert_eq!(
            inline.dependencies["test-provider"].path.as_deref(),
            Some(fs::canonicalize(directory.path()).unwrap().as_path())
        );
        assert!(child(&inline).nodes.is_empty());
        assert!(inline.flows.is_empty());
        assert_eq!(resolved.sources.len(), 1);
    }

    #[test]
    fn flow_arguments_and_inline_bodies_reject_old_versions_and_propagate_nested_errors() {
        let mut inline = definition(
            json!({"inline": {"body": {"nodes": []}}}),
            json!([call("inline")]),
        );
        inline.version = mf_runtime::WorkflowDefinitionVersion::V2026_10_28;
        let error = resolve_flows(&inline, None).err().unwrap();
        assert!(error.to_string().contains("inline Flow bodies require"));

        let mut arguments = definition(
            json!({"provider": {"builtin": "mf-compiler.resolution_test", "flows": {
                "tool": {"source": {"body": {"nodes": []}}, "description": "Tool"}
            }}}),
            json!([call("provider")]),
        );
        arguments.version = mf_runtime::WorkflowDefinitionVersion::V2026_10_28;
        let error = resolve_flows(&arguments, None).err().unwrap();
        assert!(error.to_string().contains("Flow arguments require"));

        let failing = definition(
            json!({"provider": {"builtin": "mf-compiler.resolution_test", "flows": {
                "tool": {"source": {"path": "missing.json"}, "description": "Tool"}
            }}}),
            json!([call("provider")]),
        );
        let error = resolve_flows(&failing, None).err().unwrap();
        assert!(
            error
                .to_string()
                .contains("local Flow references require load_workflow")
        );
    }

    #[test]
    fn recursive_inline_aliases_hit_the_depth_guard() {
        let authored = definition(
            json!({"recursive": {"body": {"nodes": [call("recursive")]}}}),
            json!([call("recursive")]),
        );
        let error = resolve_flows(&authored, None).err().unwrap();
        assert!(error.to_string().contains("sixteen calls"));
    }

    fn provider_definition() -> Result<WorkflowDefinition, mf_runtime::DefinitionParseError> {
        Ok(definition(json!({}), json!([])))
    }

    fn configure_provider(
        _: Value,
        mut arguments: BTreeMap<String, ResolvedFlowArgument>,
    ) -> Result<WorkflowDefinition, mf_runtime::NodeBuildError> {
        let argument = arguments.remove("tool").unwrap();
        assert_eq!(argument.description.as_deref(), Some("Tool description"));
        assert_eq!(
            argument.parameters["input"].description,
            "Input description"
        );
        assert!(argument.definition.flows.is_empty());
        Ok(argument.definition)
    }

    mf_runtime::register_builtin_flow!("resolution_test", provider_definition, configure_provider);

    fn configure_metadata_provider(
        config: Value,
        mut arguments: BTreeMap<String, mf_runtime::DescribedFlowArgument>,
    ) -> Result<WorkflowDefinition, mf_runtime::NodeBuildError> {
        assert_eq!(config, json!({"pure": true}));
        let argument = arguments.remove("tool").unwrap();
        assert!(argument.metadata.inputs.inputs.is_empty());
        assert!(argument.metadata.outputs.is_empty());
        assert!(argument.metadata.stream_outputs.is_empty());
        Ok(argument.definition)
    }

    mf_runtime::register_builtin_flow!(
        "metadata_resolution_test",
        provider_definition,
        metadata = configure_metadata_provider
    );

    #[test]
    fn metadata_expansion_is_deferred_and_frozen_after_linking() {
        let authored = definition(
            json!({"provider": {
                "builtin": "mf-compiler.metadata_resolution_test",
                "config": {"pure": true},
                "flows": {"tool": {"source": {"body": {"nodes": []}}}}
            }}),
            json!([call("provider")]),
        );
        let structural = crate::plan_definition(&authored).unwrap();
        assert_eq!(
            child(&structural.definition).nodes[0].kind,
            METADATA_EXPANSION_KIND
        );
        let serialized = structural.to_json().unwrap();
        let restored = crate::CompiledWorkflow::from_json(&serialized).unwrap();
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let finalized =
            crate::compile_resolved_definition(&restored.definition, &registry).unwrap();
        assert!(child(&finalized.definition).nodes.is_empty());
        assert!(
            !finalized
                .to_json()
                .unwrap()
                .contains(METADATA_EXPANSION_KIND)
        );
        let direct = crate::compile_definition(&authored, &registry).unwrap();
        assert_eq!(direct, finalized);
        let mut invalid = restored;
        invalid.execution_order.clear();
        assert!(crate::compiler::prepare_compiled_plan(&invalid, &registry).is_err());
        assert!(invalid.finalize(&registry).is_err());
    }

    #[test]
    fn nested_deferred_arguments_share_the_call_depth_budget() {
        let mut nested = definition(json!({}), json!([]));
        for _ in 0..=MAX_CALL_DEPTH {
            nested = definition(
                json!({}),
                json!([{
                    "id": "metadata_expansion", "kind": METADATA_EXPANSION_KIND,
                    "config": {
                        "builtin": "mf-compiler.metadata_resolution_test",
                        "config": {"pure": true},
                        "arguments": {"tool": {
                            "definition": nested, "description": null, "parameters": {}
                        }}
                    }
                }]),
            );
        }
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let error = finalize_metadata(&nested, &registry).unwrap_err();
        assert!(error.to_string().contains("sixteen calls"), "{error}");
    }

    #[test]
    fn deferred_expansion_preserves_provider_feature_requirements() {
        let mut selected = definition(json!({}), json!([]));
        selected.dependencies.insert(
            "provider".into(),
            serde_json::from_value(json!({
                "package": "test-provider", "path": "unused-host-source",
                "features": ["required"], "default-features": true
            }))
            .unwrap(),
        );
        let mut structural = crate::plan_definition(&definition(
            json!({"provider": {
                "builtin": "mf-compiler.metadata_resolution_test", "config": {"pure": true},
                "flows": {"tool": {"source": {"body": {"nodes": []}}}}
            }}),
            json!([call("provider")]),
        ))
        .unwrap();
        let mut pending = child(&structural.definition);
        pending.nodes[0].config["arguments"]["tool"]["definition"] =
            serde_json::to_value(selected).unwrap();
        pending.dependencies.insert(
            "host".into(),
            serde_json::from_value(json!({
                "package": "test-provider", "path": "different-host-source",
                "features": ["selected"], "default-features": false
            }))
            .unwrap(),
        );
        structural.definition.nodes[0].config["definition"] =
            serde_json::to_value(pending).unwrap();
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let finalized = structural.finalize(&registry).unwrap();
        let expanded = child(&finalized.definition);
        let dependency = &expanded.dependencies["provider"];
        assert_eq!(
            dependency.path.as_deref(),
            Some(Path::new("different-host-source"))
        );
        assert_eq!(dependency.features, ["required", "selected"]);
        assert!(dependency.default_features);
    }

    #[test]
    fn configurable_provider_receives_frozen_argument_and_metadata() {
        let authored = definition(
            json!({
                "provider": {"builtin": "mf-compiler.resolution_test", "flows": {
                    "tool": {
                        "source": {"body": {"nodes": [call("leaf")]}},
                        "description": "Tool description",
                        "parameters": {"input": {"description": "Input description"}}
                    }
                }},
                "leaf": {"body": {"nodes": []}}
            }),
            json!([call("provider")]),
        );
        let resolved = resolve_flows(&authored, None).unwrap();
        assert!(child(&child(&resolved.definition)).nodes.is_empty());
    }
}
