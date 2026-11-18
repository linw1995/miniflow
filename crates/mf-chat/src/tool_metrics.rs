//! Invocation-local tool execution metrics derived from traced Flow calls.

use mf_telemetry::{
    ContractError,
    events::{CALL_EXECUTION_EVENT, CallExecution, CallExecutionPhase, EventRecord},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

const MAX_TOOL_CALLS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolExecution {
    pub name: String,
    /// Missing or conflicting evidence is not a successful execution.
    pub phase: Option<CallExecutionPhase>,
    pub duration_ns: Option<u64>,
}

#[derive(Debug)]
struct ObservedExecution {
    name: String,
    attempt: Uuid,
    started: Option<u64>,
    finished: Option<(u64, CallExecutionPhase)>,
    conflicted: bool,
}

impl ObservedExecution {
    fn summary(&self, closed: bool) -> ToolExecution {
        let duration_ns = self
            .finished
            .zip(self.started)
            .and_then(|((end, _), start)| end.checked_sub(start));
        let conflicted = self.conflicted
            || (self.started.is_some() && self.finished.is_some() && duration_ns.is_none());
        ToolExecution {
            name: self.name.clone(),
            phase: if conflicted {
                None
            } else if let Some((_, phase)) = self.finished {
                Some(phase)
            } else if closed {
                None
            } else {
                Some(CallExecutionPhase::Running)
            },
            duration_ns: if conflicted { None } else { duration_ns },
        }
    }
}

#[derive(Debug, Default)]
struct Executions {
    calls: BTreeMap<String, ObservedExecution>,
    closed: bool,
}

/// A shared, bounded reader and collector for one conversation invocation's tool calls.
/// Observation never changes business results, persistence, or model context.
#[derive(Clone, Debug, Default)]
pub struct ToolExecutionMetrics(Arc<Mutex<Executions>>);

impl ToolExecutionMetrics {
    /// Accepts observation records already scoped by the caller to this run.
    pub fn observe(&self, event: &EventRecord) -> Result<(), ContractError> {
        if event.name != CALL_EXECUTION_EVENT {
            return Ok(());
        }
        let call: CallExecution = serde_json::from_value(event.body.clone())?;
        call.validate()?;
        if event.time_unix_nano == 0 {
            return mf_telemetry::InvalidSnafu {
                message: "call execution requires a source timestamp".to_owned(),
            }
            .fail();
        }
        if call.category != "tool" {
            return Ok(());
        }
        let mut state = self.0.lock().expect("tool metrics were not poisoned");
        if state.closed
            || (state.calls.len() == MAX_TOOL_CALLS
                && !state.calls.contains_key(&call.correlation_id))
        {
            return Ok(());
        }
        let observed =
            state
                .calls
                .entry(call.correlation_id)
                .or_insert_with(|| ObservedExecution {
                    name: call.name.clone(),
                    attempt: call.attempt,
                    started: None,
                    finished: None,
                    conflicted: false,
                });
        if observed.attempt != call.attempt || observed.name != call.name {
            observed.conflicted = true;
        } else if call.phase == CallExecutionPhase::Running {
            let time = event.time_unix_nano;
            observed.conflicted |= *observed.started.get_or_insert(time) != time;
        } else {
            let end = (event.time_unix_nano, call.phase);
            observed.conflicted |= *observed.finished.get_or_insert(end) != end;
        }
        Ok(())
    }

    /// Snapshot by wire tool-call ID; missing IDs have no execution evidence.
    pub fn snapshot(&self) -> BTreeMap<String, ToolExecution> {
        let state = self.0.lock().expect("tool metrics were not poisoned");
        state
            .calls
            .iter()
            .map(|(id, observed)| (id.clone(), observed.summary(state.closed)))
            .collect()
    }

    /// End observation without inferring unobserved terminal outcomes.
    pub fn close(&self) {
        self.0
            .lock()
            .expect("tool metrics were not poisoned")
            .closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CallExecutionPhase::*;

    fn event(id: &str, phase: CallExecutionPhase, time: u64) -> EventRecord {
        EventRecord {
            name: CALL_EXECUTION_EVENT.into(),
            time_unix_nano: time,
            body: serde_json::to_value(CallExecution {
                category: "tool".into(),
                correlation_id: id.into(),
                name: "lookup".into(),
                attempt: Uuid::from_u128(1),
                phase,
            })
            .unwrap(),
            trace_context: None,
        }
    }

    #[test]
    fn reordered_and_duplicate_records_preserve_one_terminal_fact() {
        let metrics = ToolExecutionMetrics::default();
        let start = event("first", Running, 10);
        let end = event("first", Succeeded, 12);
        for observation in [&end, &end, &start, &start] {
            metrics.observe(observation).unwrap();
        }
        metrics.observe(&event("other", Running, 11)).unwrap();
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot["first"].phase, Some(Succeeded));
        assert_eq!(snapshot["first"].duration_ns, Some(2));
        assert_eq!(snapshot["other"].phase, Some(Running));
        metrics.close();
        metrics.observe(&event("other", Succeeded, 13)).unwrap();
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot["first"].phase, Some(Succeeded));
        assert_eq!(snapshot["other"].phase, None);
    }

    #[test]
    fn ambiguous_or_backward_evidence_does_not_assert_a_state() {
        let metrics = ToolExecutionMetrics::default();
        let mut conflicting_attempt = event("attempt", Failed, 12);
        conflicting_attempt.body["attempt"] = serde_json::to_value(Uuid::from_u128(2)).unwrap();
        for (id, terminal) in [
            ("attempt", conflicting_attempt),
            ("clock", event("clock", Succeeded, 9)),
        ] {
            metrics.observe(&event(id, Running, 10)).unwrap();
            metrics.observe(&terminal).unwrap();
            assert_eq!(metrics.snapshot()[id].phase, None);
            assert_eq!(metrics.snapshot()[id].duration_ns, None);
        }
    }

    #[test]
    fn accepts_only_valid_tool_facts_and_bounds_retention() {
        let metrics = ToolExecutionMetrics::default();
        let mut other = event("other", Running, 10);
        other.body["category"] = "service".into();
        metrics.observe(&other).unwrap();
        let mut invalid = event("invalid", Running, 0);
        assert!(metrics.observe(&invalid).is_err());
        invalid.time_unix_nano = 1;
        invalid.body["name"] = "".into();
        assert!(metrics.observe(&invalid).is_err());
        assert!(metrics.snapshot().is_empty());
        for index in 0..=MAX_TOOL_CALLS {
            metrics
                .observe(&event(&index.to_string(), Running, 1))
                .unwrap();
        }
        metrics.observe(&event("0", Interrupted, 2)).unwrap();
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.len(), MAX_TOOL_CALLS);
        assert!(!snapshot.contains_key(&MAX_TOOL_CALLS.to_string()));
        assert_eq!(snapshot["0"].phase, Some(Interrupted));
    }
}
