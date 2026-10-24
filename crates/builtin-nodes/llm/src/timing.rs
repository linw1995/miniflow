use mf_runtime::NodeValue;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct CallTiming {
    pub elapsed_us: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_us: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, NodeValue)]
pub struct CallTokenRates {
    pub end_to_end_tokens_per_second: Option<f64>,
    pub generation_tokens_per_second: Option<f64>,
}

impl CallTiming {
    pub fn is_valid(&self) -> bool {
        self.elapsed_us >= 0
            && self
                .first_token_us
                .is_none_or(|first| first >= 0 && first <= self.elapsed_us)
    }

    pub fn generation_us(&self) -> Option<i64> {
        self.is_valid().then_some(())?;
        self.elapsed_us.checked_sub(self.first_token_us?)
    }

    pub fn token_rates(&self, output_tokens: Option<i64>) -> CallTokenRates {
        let rate = |duration: Option<i64>| {
            let tokens = output_tokens.filter(|tokens| *tokens >= 0)?;
            let duration = duration.filter(|duration| *duration > 0)?;
            Some(tokens as f64 * 1_000_000.0 / duration as f64)
        };
        if !self.is_valid() {
            return CallTokenRates::default();
        }
        CallTokenRates {
            end_to_end_tokens_per_second: rate(Some(self.elapsed_us)),
            generation_tokens_per_second: rate(self.generation_us()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_usage_keeps_unknown_timing_and_failure_rates_are_not_successes() {
        let mut usage: crate::CallUsage = serde_json::from_value(serde_json::json!({
            "call_id":"old-call", "model":"model", "usage":{"output_tokens":300}
        }))
        .unwrap();
        assert!(usage.timing.is_none());
        assert_eq!(usage.token_rates(), CallTokenRates::default());
        assert!(
            serde_json::to_value(&usage)
                .unwrap()
                .get("timing")
                .is_none()
        );
        usage.timing = Some(CallTiming {
            elapsed_us: 2_000_000,
            first_token_us: Some(500_000),
        });
        assert_eq!(
            usage.token_rates().end_to_end_tokens_per_second,
            Some(150.0)
        );
        let restored: crate::CallUsage =
            serde_json::from_slice(&serde_json::to_vec(&usage).unwrap()).unwrap();
        assert_eq!(restored, usage);
        usage.error = Some("provider failed".into());
        assert_eq!(usage.token_rates(), CallTokenRates::default());
        assert!(usage.timing.is_some());
    }

    #[test]
    fn rates_use_reported_tokens_and_distinct_observed_windows() {
        let timing = CallTiming {
            elapsed_us: 2_000_000,
            first_token_us: Some(500_000),
        };
        assert_eq!(
            timing.token_rates(Some(300)),
            CallTokenRates {
                end_to_end_tokens_per_second: Some(150.0),
                generation_tokens_per_second: Some(200.0),
            }
        );
        assert_eq!(
            timing.token_rates(Some(0)).generation_tokens_per_second,
            Some(0.0)
        );
        assert_eq!(timing.token_rates(None), CallTokenRates::default());
        assert_eq!(timing.token_rates(Some(-1)), CallTokenRates::default());
        let unary = CallTiming {
            first_token_us: None,
            ..timing.clone()
        };
        assert_eq!(
            unary.token_rates(Some(300)).end_to_end_tokens_per_second,
            Some(150.0)
        );
        assert_eq!(
            unary.token_rates(Some(300)).generation_tokens_per_second,
            None
        );
    }

    #[test]
    fn invalid_and_zero_windows_do_not_invent_rates() {
        for timing in [
            CallTiming {
                elapsed_us: -1,
                first_token_us: None,
            },
            CallTiming {
                elapsed_us: 1,
                first_token_us: Some(-1),
            },
            CallTiming {
                elapsed_us: 1,
                first_token_us: Some(2),
            },
        ] {
            assert!(!timing.is_valid());
            assert_eq!(timing.token_rates(Some(1)), CallTokenRates::default());
        }
        let zero = CallTiming {
            elapsed_us: 0,
            first_token_us: Some(0),
        };
        assert!(zero.is_valid());
        assert_eq!(zero.token_rates(Some(0)), CallTokenRates::default());
        let single = CallTiming {
            elapsed_us: 10,
            first_token_us: Some(10),
        };
        assert_eq!(
            single.token_rates(Some(1)).generation_tokens_per_second,
            None
        );
    }
}
