#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dependency<S> {
    Output {
        exclusive: bool,
        input: Option<S>,
        source_node: S,
        source_output: S,
    },
    NodeSuccess {
        source_node: S,
    },
}

pub type ExecutionDependency<'a> = Dependency<&'a str>;

impl Dependency<std::borrow::Cow<'static, str>> {
    pub fn borrowed(&self) -> ExecutionDependency<'_> {
        match self {
            Self::Output {
                exclusive,
                input,
                source_node,
                source_output,
            } => Dependency::Output {
                exclusive: *exclusive,
                input: input.as_ref().map(AsRef::as_ref),
                source_node: source_node.as_ref(),
                source_output: source_output.as_ref(),
            },
            Self::NodeSuccess { source_node } => Dependency::NodeSuccess {
                source_node: source_node.as_ref(),
            },
        }
    }
}

impl<'a> ExecutionDependency<'a> {
    pub fn output_dependency(self) -> Option<mf_runtime::ExecutionDependency<'a>> {
        match self {
            Self::Output {
                exclusive,
                input,
                source_node,
                source_output,
            } => Some(mf_runtime::ExecutionDependency {
                exclusive,
                input,
                source_node,
                source_output,
                source_key: std::borrow::Cow::Owned(mf_runtime::output_id(
                    source_node,
                    source_output,
                )),
            }),
            Self::NodeSuccess { .. } => None,
        }
    }

    pub fn source_node(self) -> &'a str {
        match self {
            Self::Output { source_node, .. } | Self::NodeSuccess { source_node } => source_node,
        }
    }
    pub fn source_output(self) -> Option<&'a str> {
        match self {
            Self::Output { source_output, .. } => Some(source_output),
            Self::NodeSuccess { .. } => None,
        }
    }
    pub fn input(self) -> Option<&'a str> {
        match self {
            Self::Output { input, .. } => input,
            Self::NodeSuccess { .. } => None,
        }
    }
    pub fn exclusive(self) -> bool {
        matches!(
            self,
            Self::Output {
                exclusive: true,
                ..
            }
        )
    }
}

pub type FlowDependency = Dependency<std::borrow::Cow<'static, str>>;

impl FlowDependency {
    pub fn output_dependency(&self) -> Option<mf_runtime::FlowDependency> {
        match self {
            Self::Output {
                exclusive,
                input,
                source_node,
                source_output,
            } => Some(mf_runtime::FlowDependency {
                exclusive: *exclusive,
                input: input.clone(),
                source_node: source_node.clone(),
                source_output: source_output.clone(),
                source_key: mf_runtime::output_id(source_node, source_output).into(),
            }),
            Self::NodeSuccess { .. } => None,
        }
    }
}

use mf_runtime::{ExecutionContext, NodeExecutionError, SkipCause, WorkflowRunError};
use snafu::ResultExt;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

type Clause = BTreeSet<(String, String)>;

pub struct SuccessCondition {
    pub source: String,
    pub clauses: BTreeSet<Clause>,
}

pub fn success_conditions<'a>(
    ordered: impl IntoIterator<Item = (&'a str, Vec<ExecutionDependency<'a>>)>,
) -> Vec<Vec<SuccessCondition>> {
    let mut activations: BTreeMap<&str, BTreeSet<Clause>> = BTreeMap::new();
    let mut guards = Vec::new();
    for (id, dependencies) in ordered {
        let mut activation = BTreeSet::new();
        let mut exclusive: BTreeMap<&str, Clause> = BTreeMap::new();
        let mut prerequisites = Vec::new();
        for dependency in dependencies {
            match dependency {
                Dependency::Output {
                    exclusive: true,
                    input: Some(input),
                    source_node,
                    source_output,
                } => {
                    exclusive
                        .entry(input)
                        .or_default()
                        .insert((source_node.into(), source_output.into()));
                }
                Dependency::Output {
                    source_node,
                    source_output,
                    ..
                } => {
                    activation.insert(BTreeSet::from([(source_node.into(), source_output.into())]));
                }
                Dependency::NodeSuccess { source_node } => {
                    let clauses = activations
                        .get(source_node)
                        .expect("validated source precedes target")
                        .clone();
                    activation.extend(clauses.iter().cloned());
                    prerequisites.push(SuccessCondition {
                        source: source_node.into(),
                        clauses,
                    });
                }
            }
        }
        activation.extend(exclusive.into_values());
        activations.insert(id, activation);
        guards.push(prerequisites);
    }
    guards
}

type Predicate = Box<dyn Fn(&ExecutionContext) -> Result<bool, NodeExecutionError> + Send + Sync>;
type Guard =
    Box<dyn Fn(&ExecutionContext) -> Result<BTreeSet<SkipCause>, WorkflowRunError> + Send + Sync>;

// Preparation binds the predicate structure once; execution does not walk a dependency graph.
fn bind_condition(clauses: BTreeSet<Clause>) -> Predicate {
    let mut conjunction: Predicate = Box::new(|_| Ok(true));
    for clause in clauses {
        let mut disjunction: Predicate = Box::new(|_| Ok(false));
        for (source, port) in clause {
            let key = mf_runtime::output_id(&source, &port);
            let previous = disjunction;
            disjunction = Box::new(move |ctx| Ok(previous(ctx)? | ctx.output_available_key(&key)?));
        }
        let previous = conjunction;
        conjunction = Box::new(move |ctx| Ok(previous(ctx)? & disjunction(ctx)?));
    }
    conjunction
}

fn bind_guards(id: String, conditions: Vec<SuccessCondition>) -> Guard {
    let mut guard: Guard = Box::new(|_| Ok(BTreeSet::new()));
    for condition in conditions {
        let predicate = bind_condition(condition.clauses);
        let source = condition.source;
        let id = id.clone();
        let previous = guard;
        guard = Box::new(move |ctx| {
            let mut causes = previous(ctx)?;
            let active = (|| {
                if let Some(error) = ctx.cancellation().failure() {
                    return Err(error.into());
                }
                predicate(ctx)
            })()
            .context(mf_runtime::SuccessDependencySnafu {
                definition_id: id.as_str(),
                source_node: source.as_str(),
            })?;
            if !active {
                causes.insert(SkipCause::NodeSuccess {
                    node_success: source.clone(),
                });
            }
            Ok(causes)
        });
    }
    guard
}

pub fn bind_task_guards(
    flow: mf_runtime::Flow,
    guards: Vec<Vec<SuccessCondition>>,
) -> mf_runtime::Flow {
    let guards: Vec<_> = guards
        .into_iter()
        .enumerate()
        .filter_map(|(position, conditions)| {
            if conditions.is_empty() {
                return None;
            }
            let node = flow.execution_order()[position];
            let guard: Arc<mf_runtime::ExecutionGuard> = Arc::from(bind_guards(
                flow.nodes()[node.index()].definition_id.to_string(),
                conditions,
            ));
            Some((node, guard))
        })
        .collect();
    flow.with_input_guards(guards)
}

pub fn bind_stream_guards(
    prepared: mf_runtime::PreparedStream,
    guards: Vec<Vec<SuccessCondition>>,
) -> mf_runtime::PreparedStream {
    let mut guards: Vec<_> = guards
        .into_iter()
        .enumerate()
        .map(|(position, conditions)| {
            (!conditions.is_empty()).then(|| {
                bind_guards(
                    prepared.plan().nodes()[position].definition_id.to_string(),
                    conditions,
                )
            })
        })
        .collect();
    let mut executors = Vec::new();
    for domain in prepared.plan().execution_domains().domains() {
        if !domain
            .positions
            .iter()
            .any(|&position| guards[position].is_some())
        {
            continue;
        }
        let positions: Vec<_> = domain
            .positions
            .iter()
            .map(|&position| (position, guards[position].take()))
            .collect();
        let executor: Arc<mf_runtime::GeneratedStreamDomainExecutor> =
            Arc::new(move |plan, ctx| {
                for (position, guard) in &positions {
                    if ctx.cancellation().failure().is_some() || ctx.scope_exit_requested() {
                        break;
                    }
                    plan.execute_resolved_position(*position, ctx, |node, ctx, inputs| {
                        let causes = guard
                            .as_ref()
                            .map_or_else(|| Ok(BTreeSet::new()), |guard| guard(ctx))?;
                        let _ = node;
                        plan.resolve_inputs(*position, ctx, inputs, causes)
                    })?;
                }
                Ok(())
            });
        executors.push((domain.id, executor));
    }
    prepared.with_prepared_domains(executors)
}
