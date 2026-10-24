//! Derives model request timing exclusively from source event timestamps.

use crate::{CallTiming, CallUsage};
use mf_telemetry::{events::EventRecord, wire::TraceContext};
use serde::Deserialize;
use snafu::{ResultExt, Snafu, ensure};
use std::collections::BTreeMap;

#[derive(Debug, Snafu)]
pub enum CallEventError {
    #[snafu(display("invalid model call event payload: {source}"))]
    Payload { source: serde_json::Error },
    #[snafu(display("invalid model call facts: {source}"))]
    Facts { source: crate::ChatError },
    #[snafu(display("invalid model call event: {reason}"))]
    Invalid { reason: &'static str },
}

#[derive(Clone, Debug, Default)]
struct ObservedCall {
    started: Option<(u64, CallUsage)>,
    first_text: Option<u64>,
    finished: Option<(u64, CallUsage)>,
    correlation: Option<TraceContext>,
    conflicted: bool,
}

impl ObservedCall {
    fn timing(&self) -> Option<CallTiming> {
        if self.conflicted {
            return None;
        }
        let start = self.started.as_ref()?.0;
        let end = self.finished.as_ref()?.0;
        let elapsed_us = i64::try_from(end.checked_sub(start)? / 1000).ok()?;
        let first_token_us = match self.first_text {
            Some(first) => {
                if first > end {
                    return None;
                }
                Some(i64::try_from(first.checked_sub(start)? / 1000).ok()?)
            }
            None => None,
        };
        Some(CallTiming {
            elapsed_us,
            first_token_us,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct CallEventCollector {
    calls: BTreeMap<String, ObservedCall>,
}

impl CallEventCollector {
    pub fn recognizes(name: &str) -> bool {
        matches!(
            name,
            mfn_llm::REQUEST_STARTED_EVENT
                | mfn_llm::FIRST_TEXT_EVENT
                | mfn_llm::REQUEST_FINISHED_EVENT
        )
    }

    pub fn observe(&mut self, event: &EventRecord) -> Result<(), CallEventError> {
        if !Self::recognizes(&event.name) {
            return Ok(());
        }
        ensure!(
            event.time_unix_nano > 0,
            InvalidSnafu {
                reason: "requires a source timestamp"
            }
        );
        #[derive(Deserialize)]
        struct Identity {
            call_id: String,
        }
        let id: Identity = serde_json::from_value(event.body.clone()).context(PayloadSnafu)?;
        ensure!(
            !id.call_id.trim().is_empty(),
            InvalidSnafu {
                reason: "requires a call identity"
            }
        );
        let usage = if event.name == mfn_llm::FIRST_TEXT_EVENT {
            None
        } else {
            let facts: mfn_llm::CallUsage =
                serde_json::from_value(event.body.clone()).context(PayloadSnafu)?;
            let usage = CallUsage::from(facts);
            crate::turn::validate_usages(std::slice::from_ref(&usage)).context(FactsSnafu)?;
            Some(usage)
        };
        let call = self.calls.entry(id.call_id).or_default();
        if let Some(context) = &event.trace_context
            && (event.name == mfn_llm::REQUEST_STARTED_EVENT || call.correlation.is_none())
        {
            call.correlation = Some(context.clone());
        }
        if let Some(usage) = usage {
            let slot = if event.name == mfn_llm::REQUEST_STARTED_EVENT {
                &mut call.started
            } else {
                &mut call.finished
            };
            if let Some(previous) = slot {
                call.conflicted |= previous != &(event.time_unix_nano, usage);
            } else {
                *slot = Some((event.time_unix_nano, usage));
            }
        } else if let Some(previous) = call.first_text {
            call.conflicted |= previous != event.time_unix_nano;
        } else {
            call.first_text = Some(event.time_unix_nano);
        }
        if let (Some((_, start)), Some((_, end))) = (&call.started, &call.finished) {
            call.conflicted |= start.model != end.model
                || start.node != end.node
                || start.attribution != end.attribution;
        }
        Ok(())
    }

    pub fn apply(&self, usages: &mut [CallUsage]) {
        for usage in usages {
            if let Some(call) = self.calls.get(&usage.call_id) {
                usage.timing = call.timing();
                if let Some((timestamp, _)) = &call.started {
                    usage.started_at = i64::try_from(timestamp / 1_000_000).ok();
                }
            }
        }
    }

    pub fn usages_for_span(&self, trace: &str, span: &str) -> Vec<CallUsage> {
        let mut usages = self
            .calls
            .values()
            .filter(|call| {
                call.correlation
                    .as_ref()
                    .is_some_and(|context| context.trace_id == trace && context.span_id == span)
            })
            .filter_map(|call| {
                call.finished
                    .as_ref()
                    .or(call.started.as_ref())
                    .map(|(_, usage)| usage.clone())
            })
            .collect::<Vec<_>>();
        self.apply(&mut usages);
        usages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn usage(id: &str) -> CallUsage {
        serde_json::from_value(
            json!({"call_id":id,"model":"fixture","usage":{"output_tokens":100}}),
        )
        .unwrap()
    }
    fn event(name: &str, time: u64, id: &str) -> EventRecord {
        EventRecord {
            name: name.into(),
            time_unix_nano: time,
            body: serde_json::to_value(usage(id)).unwrap(),
            trace_context: None,
        }
    }
    fn records(id: &str) -> [EventRecord; 3] {
        [
            event(mfn_llm::REQUEST_STARTED_EVENT, 10_000_000_000, id),
            event(mfn_llm::FIRST_TEXT_EVENT, 10_500_000_000, id),
            event(mfn_llm::REQUEST_FINISHED_EVENT, 12_000_000_000, id),
        ]
    }

    #[test]
    fn source_timestamps_survive_reordering_duplicates_and_repeated_calls() {
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let mut collector = CallEventCollector::default();
            for id in ["one", "two"] {
                let records = records(id);
                for index in order {
                    collector.observe(&records[index]).unwrap();
                }
                for record in &records {
                    collector.observe(record).unwrap();
                }
            }
            let mut usages = [usage("two"), usage("one")];
            collector.apply(&mut usages);
            for usage in usages {
                assert_eq!(usage.started_at, Some(10_000));
                assert_eq!(
                    usage.timing,
                    Some(CallTiming {
                        elapsed_us: 2_000_000,
                        first_token_us: Some(500_000)
                    })
                );
                assert_eq!(usage.token_rates().end_to_end_tokens_per_second, Some(50.0));
                assert!(
                    (usage.token_rates().generation_tokens_per_second.unwrap() - 100.0 / 1.5).abs()
                        < 1e-9
                );
            }
        }
    }

    #[test]
    fn incomplete_conflicting_and_backward_clocks_remain_unknown() {
        let complete = records("one");
        for indices in [vec![], vec![0], vec![1], vec![2], vec![0, 1], vec![1, 2]] {
            let mut collector = CallEventCollector::default();
            for index in indices {
                collector.observe(&complete[index]).unwrap();
            }
            let mut usages = [usage("one")];
            collector.apply(&mut usages);
            assert!(usages[0].timing.is_none());
        }
        let mut unary = CallEventCollector::default();
        unary.observe(&complete[0]).unwrap();
        unary.observe(&complete[2]).unwrap();
        let mut usages = [usage("one")];
        unary.apply(&mut usages);
        assert_eq!(usages[0].timing.as_ref().unwrap().first_token_us, None);
        for (first, end) in [
            (9_000_000_000, 12_000_000_000),
            (13_000_000_000, 12_000_000_000),
            (10_500_000_000, 9_000_000_000),
        ] {
            let mut collector = CallEventCollector::default();
            for record in [
                complete[0].clone(),
                event(mfn_llm::FIRST_TEXT_EVENT, first, "one"),
                event(mfn_llm::REQUEST_FINISHED_EVENT, end, "one"),
            ] {
                collector.observe(&record).unwrap();
            }
            let mut usages = [usage("one")];
            collector.apply(&mut usages);
            assert!(usages[0].timing.is_none());
        }
        let mut collector = CallEventCollector::default();
        for record in &complete {
            collector.observe(record).unwrap();
        }
        collector
            .observe(&event(
                mfn_llm::REQUEST_FINISHED_EVENT,
                13_000_000_000,
                "one",
            ))
            .unwrap();
        let mut usages = [usage("one")];
        collector.apply(&mut usages);
        assert!(usages[0].timing.is_none());
    }

    #[test]
    fn span_lookup_uses_request_start_and_retains_failed_facts() {
        let mut collector = CallEventCollector::default();
        let mut records = records("one");
        records[0].trace_context = Some(TraceContext {
            trace_id: "trace".into(),
            span_id: "node".into(),
            trace_flags: 1,
        });
        records[1].trace_context = Some(TraceContext {
            trace_id: "trace".into(),
            span_id: "nested-provider".into(),
            trace_flags: 1,
        });
        records[2].body["error"] = json!("provider failed");
        for record in records.iter().rev() {
            collector.observe(record).unwrap();
        }
        let usages = collector.usages_for_span("trace", "node");
        assert_eq!(usages.len(), 1);
        assert!(usages[0].timing.is_some());
        assert_eq!(usages[0].token_rates(), crate::CallTokenRates::default());
        assert!(
            collector
                .usages_for_span("trace", "nested-provider")
                .is_empty()
        );
    }

    #[test]
    fn malformed_events_preserve_typed_sources_and_legacy_facts() {
        use std::error::Error as _;
        let mut collector = CallEventCollector::default();
        collector
            .observe(&event("future.observation", 0, ""))
            .unwrap();
        assert!(
            collector
                .observe(&event(mfn_llm::FIRST_TEXT_EVENT, 1, " "))
                .is_err()
        );
        let mut invalid = event(mfn_llm::REQUEST_STARTED_EVENT, 0, "one");
        assert!(collector.observe(&invalid).is_err());
        invalid.time_unix_nano = 1;
        invalid.body = json!({});
        let error = collector.observe(&invalid).unwrap_err();
        assert!(error.source().unwrap().is::<serde_json::Error>());
        invalid.body = serde_json::to_value(usage("one")).unwrap();
        invalid.body["model"] = json!("");
        let error = collector.observe(&invalid).unwrap_err();
        assert!(error.source().unwrap().is::<crate::ChatError>());
        let mut legacy = usage("legacy");
        legacy.timing = Some(CallTiming {
            elapsed_us: 1,
            first_token_us: None,
        });
        collector.apply(std::slice::from_mut(&mut legacy));
        assert!(legacy.timing.is_some());
    }
}
