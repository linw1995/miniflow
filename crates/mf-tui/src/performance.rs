//! Shared presentation of persisted and observed model call facts.

use crate::{receiver::ObservationReader, state::NodeObservation};
use mf_chat::{CallUsage, UsageSummary};

pub fn node_calls(reader: &ObservationReader, node: &NodeObservation) -> Vec<CallUsage> {
    let Some((trace, span)) = &node.correlation else {
        return vec![];
    };
    let data = reader.span_data(trace, span);
    let mut usages: Vec<CallUsage> = data
        .node
        .as_ref()
        .and_then(|node| node.outputs.get("usages"))
        .and_then(|value| serde_json::to_value(value).ok())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    if usages.is_empty() {
        usages = reader.calls_for_span(trace, span);
    }
    reader.apply_call_timings(&mut usages);
    usages
}

pub fn conversation_calls(calls: &[CallUsage]) -> Vec<CallUsage> {
    calls
        .iter()
        .filter(|call| {
            call.attribution
                .as_ref()
                .and_then(|attribution| attribution.purpose.as_deref())
                .is_none_or(|purpose| purpose == "conversation")
        })
        .cloned()
        .collect()
}

fn latency(value: Option<f64>) -> String {
    value.map_or_else(
        || "—".into(),
        |ms| {
            if ms < 1000.0 {
                format!("{ms:.0}ms")
            } else {
                format!("{:.2}s", ms / 1000.0)
            }
        },
    )
}

fn rate(value: Option<f64>) -> String {
    value.map_or_else(|| "—".into(), |value| format!("{value:.1}"))
}

pub fn card_lines(calls: &[CallUsage]) -> [String; 2] {
    let summary = UsageSummary::from_calls(calls).ok();
    let performance = summary.as_ref().map(|summary| &summary.performance);
    let count = calls.len();
    let prefix = if count > 1 { "Avg FTT" } else { "FTT" };
    let coverage = performance
        .filter(|_| count > 1)
        .map_or(String::new(), |p| {
            format!(" | {}/{} calls", p.first_token.reported_calls, count)
        });
    [
        format!(
            "{prefix} {}{coverage}",
            latency(performance.and_then(|p| p.first_token.mean_ms))
        ),
        format!(
            "Gen {} | E2E {} tok/s",
            rate(performance.and_then(|p| p.generation.tokens_per_second)),
            rate(performance.and_then(|p| p.end_to_end.tokens_per_second))
        ),
    ]
}

pub fn footer(calls: &[CallUsage], width: u16) -> String {
    let [first, rates] = card_lines(calls);
    if width < 60 {
        format!(
            "{first} · {} tok/s",
            rates.split(" | ").next().unwrap_or("Gen —")
        )
    } else {
        format!("{first} · {rates}")
    }
}

pub fn details(calls: &[CallUsage]) -> String {
    let [first, rates] = card_lines(calls);
    let mut text = format!(
        "{first}\n{rates}\nFTT: request to first nonempty text delta.\nGen: output tokens / (request time - FTT).\nE2E: output tokens / request time.\nRates use provider output counts; unknown values are —.\nMultiple calls use mean FTT and weighted rates.\n"
    );
    if let Ok(summary) = UsageSummary::from_calls(calls) {
        text.push_str(&format!(
            "Rate coverage: Gen {}/{}, E2E {}/{} calls\n",
            summary.performance.generation.reported_calls,
            calls.len(),
            summary.performance.end_to_end.reported_calls,
            calls.len()
        ));
    }
    for call in calls {
        // Model and attribution labels are untrusted provider/workflow text.
        let clean = |text: &str| text.chars().filter(|c| !c.is_control()).collect::<String>();
        let purpose = call
            .attribution
            .as_ref()
            .and_then(|a| a.purpose.as_deref())
            .unwrap_or("unspecified");
        let [first, rates] = card_lines(std::slice::from_ref(call));
        let tokens = |name| {
            call.usage
                .as_ref()
                .and_then(|usage| usage.get(name))
                .map_or_else(|| "—".into(), ToString::to_string)
        };
        let elapsed = latency(
            call.timing
                .as_ref()
                .filter(|timing| timing.is_valid())
                .map(|timing| timing.elapsed_us as f64 / 1000.0),
        );
        text.push_str(&format!(
            "Call {}: {} / {}{}\nTokens: in {} / out {} | Request {elapsed}\n{first}\n{rates}\n",
            clean(&call.call_id),
            clean(&call.model),
            clean(purpose),
            if call.error.is_some() {
                " [failed]"
            } else {
                ""
            },
            tokens("input_tokens"),
            tokens("output_tokens"),
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, elapsed: i64, first: Option<i64>, tokens: Option<i64>) -> CallUsage {
        serde_json::from_value(serde_json::json!({
            "call_id": id, "model": "fixture", "timing": {"elapsed_us": elapsed, "first_token_us": first},
            "usage": tokens.map(|tokens| serde_json::json!({"output_tokens": tokens}))
        })).unwrap()
    }

    #[test]
    fn summaries_weight_rates_and_report_partial_first_token_coverage() {
        let calls = vec![
            call("a", 2_000_000, Some(500_000), Some(100)),
            call("b", 4_000_000, Some(1_000_000), Some(100)),
            call("c", 1_000_000, None, Some(60)),
        ];
        assert_eq!(
            card_lines(&calls),
            ["Avg FTT 750ms | 2/3 calls", "Gen 44.4 | E2E 37.1 tok/s"]
        );
        assert!(!footer(&calls, 45).contains("E2E"));
        assert!(footer(&calls, 90).contains("E2E 37.1"));
        assert!(details(&calls).contains("Rate coverage: Gen 2/3, E2E 3/3 calls"));
        assert!(details(&calls).contains("Call c: fixture / unspecified"));
    }

    #[test]
    fn unknown_failed_unary_and_zero_windows_are_not_estimated() {
        assert_eq!(card_lines(&[]), ["FTT —", "Gen — | E2E — tok/s"]);
        let unary = call("unary", 1_000_000, None, Some(0));
        assert_eq!(card_lines(&[unary]), ["FTT —", "Gen — | E2E 0.0 tok/s"]);
        let mut failed = call("failed", 2_000_000, Some(500_000), Some(100));
        failed.error = Some("provider failed".into());
        assert_eq!(card_lines(&[failed]), ["FTT 500ms", "Gen — | E2E — tok/s"]);
        assert_eq!(
            card_lines(&[call("zero", 0, Some(0), Some(10))]),
            ["FTT 0ms", "Gen — | E2E — tok/s"]
        );
        assert_eq!(
            card_lines(&[call("invalid", 10, Some(20), Some(10))]),
            ["FTT —", "Gen — | E2E — tok/s"]
        );
        assert_eq!(
            card_lines(&[call("sparse", 2_000_000, Some(1_000_000), None)]),
            ["FTT 1.00s", "Gen — | E2E — tok/s"]
        );
    }

    #[test]
    fn reply_summary_excludes_title_and_other_explicit_purposes() {
        let mut title = call("title", 100, Some(10), Some(10));
        title.attribution = Some(mf_chat::UsageAttribution {
            purpose: Some("title".into()),
            profile: None,
        });
        let answer = call("answer", 2_000_000, Some(500_000), Some(100));
        assert_eq!(conversation_calls(&[title, answer.clone()]), vec![answer]);
    }
}
