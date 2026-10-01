use mf_runtime::{ExecutionScope, ScopeNodeObservation, ScopeObserver};
use mf_telemetry::{
    Count,
    event::{LoopPassOutcome, LoopPathEntry},
    observation::{BodyObservation, RunObservation},
};

pub fn loop_path(scopes: &[ExecutionScope]) -> Vec<LoopPathEntry> {
    scopes
        .iter()
        .filter(|scope| scope.is_observed())
        .map(|scope| LoopPathEntry {
            loop_id: scope.node_id().into(),
            index: Count::try_from(scope.index() as i64).expect("scope index is bounded"),
        })
        .collect()
}

#[derive(Debug)]
pub struct LoopScopeObserver;

impl ScopeObserver for LoopScopeObserver {
    fn started(&self, run: Option<&mut RunObservation>, path: &[ExecutionScope]) {
        if let Some(run) = run.filter(|run| run.supports_loops()) {
            run.loop_pass_started(loop_path(path));
        }
    }

    fn finished(&self, run: Option<&mut RunObservation>, path: &[ExecutionScope], failed: bool) {
        if let Some(run) = run.filter(|run| run.supports_loops()) {
            let scope = path.last().expect("active scope");
            let outcome = if failed {
                LoopPassOutcome::Failed
            } else if scope.exited() {
                LoopPassOutcome::Exit
            } else {
                LoopPassOutcome::Completed
            };
            let visited =
                Count::try_from(scope.visited_steps() as i64).expect("scope budget bounds visits");
            run.loop_pass_finished(loop_path(path), visited, outcome);
        }
    }

    fn begin_node(
        &self,
        run: Option<&mut RunObservation>,
        path: &[ExecutionScope],
        id: &str,
    ) -> Option<ScopeNodeObservation> {
        run?.begin_invocation(loop_path(path), id)
            .map(ScopeNodeObservation::Root)
    }
}

#[derive(Debug)]
pub struct ItemScopeObserver(pub BodyObservation);

impl ScopeObserver for ItemScopeObserver {
    fn begin_node(
        &self,
        _: Option<&mut RunObservation>,
        _: &[ExecutionScope],
        id: &str,
    ) -> Option<ScopeNodeObservation> {
        self.0.begin_node(id).map(ScopeNodeObservation::Body)
    }
}
