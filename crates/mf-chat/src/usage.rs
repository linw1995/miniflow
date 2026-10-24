use crate::{
    ChatDatabase, ChatError,
    error::*,
    schema::{model_calls as invocations, usage_facts as model_calls},
    session, storage,
};
use diesel::{
    dsl::{count, count_star, sql},
    prelude::*,
    sql_types::{BigInt, Nullable, Text},
    sqlite::Sqlite,
};
use mf_runtime::{NodeEnum, NodeValue};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, ensure};
use std::{collections::BTreeMap, path::Path};

#[cfg(test)]
mod tests;

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

impl crate::CallUsage {
    pub fn token_rates(&self) -> CallTokenRates {
        if self.error.is_some() {
            return CallTokenRates::default();
        }
        self.timing
            .as_ref()
            .map_or_else(CallTokenRates::default, |timing| {
                timing.token_rates(
                    self.usage
                        .as_ref()
                        .and_then(|usage| usage.get("output_tokens").copied()),
                )
            })
    }
}

#[diesel::declare_sql_function]
extern "SQL" {
    #[aggregate]
    #[sql_name = "SUM"]
    fn sum_i64(value: Nullable<BigInt>) -> Nullable<BigInt>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, NodeEnum)]
#[serde(rename_all = "snake_case")]
pub enum CallPurpose {
    #[value(rename = "conversation")]
    Conversation,
    #[value(rename = "title")]
    Title,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, NodeEnum)]
#[serde(rename_all = "snake_case")]
pub enum UsageGrouping {
    #[value(rename = "session")]
    Session,
    #[value(rename = "model")]
    Model,
    #[value(rename = "profile")]
    Profile,
    #[value(rename = "purpose")]
    Purpose,
    #[value(rename = "day")]
    Day,
}

#[derive(Clone, Debug, Serialize, Deserialize, NodeValue)]
pub struct UsageQuery {
    pub session_id: Option<String>,
    pub profile: Option<String>,
    pub model: Option<String>,
    pub purpose: Option<CallPurpose>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub group_by: Option<UsageGrouping>,
    #[value(default = "session::default_page_size")]
    #[serde(default = "session::default_page_size")]
    pub limit: usize,
    pub after: Option<String>,
}
impl Default for UsageQuery {
    fn default() -> Self {
        Self {
            session_id: None,
            profile: None,
            model: None,
            purpose: None,
            start_ms: None,
            end_ms: None,
            group_by: None,
            limit: session::default_page_size(),
            after: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct TokenCounter {
    pub known_subtotal: Option<i64>,
    pub reported_calls: i64,
    pub missing_calls: i64,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, NodeValue)]
pub struct LatencySummary {
    pub total_us: Option<i64>,
    pub mean_ms: Option<f64>,
    pub reported_calls: i64,
    pub missing_calls: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, NodeValue)]
pub struct TokenRateSummary {
    pub output_tokens: Option<i64>,
    pub elapsed_us: Option<i64>,
    pub tokens_per_second: Option<f64>,
    pub reported_calls: i64,
    pub missing_calls: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, NodeValue)]
pub struct PerformanceSummary {
    pub elapsed: LatencySummary,
    pub first_token: LatencySummary,
    pub end_to_end: TokenRateSummary,
    pub generation: TokenRateSummary,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, NodeValue)]
pub struct UsageSummary {
    pub attempted_calls: i64,
    pub statuses: BTreeMap<String, i64>,
    pub input_tokens: TokenCounter,
    pub output_tokens: TokenCounter,
    pub total_tokens: TokenCounter,
    #[value(default)]
    #[serde(default)]
    pub performance: PerformanceSummary,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, NodeValue)]
pub struct UsageGroup {
    pub key: String,
    pub summary: UsageSummary,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, NodeValue)]
pub struct UsageStatistics {
    pub unreported_invocations: i64,
    pub overall: UsageSummary,
    pub groups: Vec<UsageGroup>,
    pub next_after: Option<String>,
}
#[derive(Clone, Debug, NodeValue)]
pub struct UsageInputs {
    pub database_path: String,
    pub query: UsageQuery,
}
#[derive(Clone, Debug, NodeValue)]
pub struct UsageOutputs {
    pub statistics: UsageStatistics,
}

type TokenMeasures = (i64, Option<i64>, i64, Option<i64>, i64, Option<i64>, i64);
type LatencyMeasures = (Option<i64>, i64, Option<i64>, i64);
type RateMeasures = (Option<i64>, Option<i64>, i64, Option<i64>, Option<i64>, i64);
type Measures = (TokenMeasures, LatencyMeasures, RateMeasures);

macro_rules! measures {
    () => {{
        let end_tokens = sql::<Nullable<BigInt>>("CASE WHEN state = 'succeeded' AND elapsed_us > 0 THEN output_tokens END");
        let end_time = sql::<Nullable<BigInt>>("CASE WHEN state = 'succeeded' AND elapsed_us > 0 AND output_tokens IS NOT NULL THEN elapsed_us END");
        let generation_tokens = sql::<Nullable<BigInt>>("CASE WHEN state = 'succeeded' AND first_token_us >= 0 AND elapsed_us > first_token_us THEN output_tokens END");
        let generation_time = sql::<Nullable<BigInt>>("CASE WHEN state = 'succeeded' AND first_token_us >= 0 AND elapsed_us > first_token_us AND output_tokens IS NOT NULL THEN elapsed_us - first_token_us END");
        (
            (count_star(),
             sum_i64(model_calls::input_tokens), count(model_calls::input_tokens),
             sum_i64(model_calls::output_tokens), count(model_calls::output_tokens),
             sum_i64(model_calls::total_tokens), count(model_calls::total_tokens)),
            (sum_i64(model_calls::elapsed_us), count(model_calls::elapsed_us),
             sum_i64(model_calls::first_token_us), count(model_calls::first_token_us)),
            (sum_i64(end_tokens.clone()), sum_i64(end_time), count(end_tokens),
             sum_i64(generation_tokens.clone()), sum_i64(generation_time), count(generation_tokens)),
        )
    }};
}

fn add_observed(left: Option<i64>, right: Option<i64>) -> Result<Option<i64>, ChatError> {
    right.map_or(Ok(left), |value| {
        left.unwrap_or(0)
            .checked_add(value)
            .map(Some)
            .context(ArithmeticSnafu {
                operation: "sum observed performance",
            })
    })
}

fn observation_counts(
    reported: &mut i64,
    additional: i64,
    attempted: i64,
) -> Result<i64, ChatError> {
    *reported = reported.checked_add(additional).context(ArithmeticSnafu {
        operation: "count performance observations",
    })?;
    attempted.checked_sub(*reported).context(ArithmeticSnafu {
        operation: "count missing performance observations",
    })
}

impl LatencySummary {
    fn add(
        &mut self,
        duration: Option<i64>,
        reported: i64,
        attempted: i64,
    ) -> Result<(), ChatError> {
        self.total_us = add_observed(self.total_us, duration)?;
        self.missing_calls = observation_counts(&mut self.reported_calls, reported, attempted)?;
        self.mean_ms = self
            .total_us
            .filter(|_| self.reported_calls > 0)
            .map(|total| total as f64 / self.reported_calls as f64 / 1000.0);
        Ok(())
    }
}

impl TokenRateSummary {
    fn add(
        &mut self,
        tokens: Option<i64>,
        duration: Option<i64>,
        reported: i64,
        attempted: i64,
    ) -> Result<(), ChatError> {
        self.output_tokens = add_observed(self.output_tokens, tokens)?;
        self.elapsed_us = add_observed(self.elapsed_us, duration)?;
        self.missing_calls = observation_counts(&mut self.reported_calls, reported, attempted)?;
        self.tokens_per_second = self
            .output_tokens
            .zip(self.elapsed_us)
            .filter(|(_, duration)| *duration > 0)
            .map(|(tokens, duration)| tokens as f64 * 1_000_000.0 / duration as f64);
        Ok(())
    }
}
macro_rules! filtered {
    ($statement:expr, $filter:expr) => {
        filtered!($statement, $filter, model_calls)
    };
    ($statement:expr, $filter:expr, $source:ident) => {{
        let filter = $filter;
        let mut statement = $statement
            .into_boxed::<Sqlite>()
            .filter($source::started_at.is_not_null());
        if let Some(value) = &filter.session_id {
            statement = statement.filter($source::session_id.eq(value));
        }
        if let Some(value) = &filter.profile {
            statement = statement.filter($source::profile.eq(value));
        }
        if let Some(value) = &filter.model {
            statement = statement.filter($source::model.eq(value));
        }
        if let Some(value) = filter.purpose {
            statement = statement.filter($source::purpose.eq(value.as_str()));
        }
        if let Some(value) = filter.start_ms {
            statement = statement.filter($source::started_at.ge(value));
        }
        if let Some(value) = filter.end_ms {
            statement = statement.filter($source::started_at.lt(value));
        }
        statement
    }};
}

impl CallPurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Title => "title",
        }
    }
}

impl UsageSummary {
    /// Aggregates call facts with the same paired, weighted rates as persisted queries.
    pub fn from_calls(calls: &[crate::CallUsage]) -> Result<Self, ChatError> {
        crate::turn::validate_usages(calls)?;
        let mut summary = Self::empty();
        for call in calls {
            let count = |key| {
                call.usage
                    .as_ref()
                    .and_then(|usage| usage.get(key))
                    .copied()
            };
            let input = count("input_tokens");
            let output = count("output_tokens");
            let total = count("total_tokens");
            let timing = call.timing.as_ref();
            let elapsed = timing.map(|timing| timing.elapsed_us);
            let first = timing.and_then(|timing| timing.first_token_us);
            let rates = call.token_rates();
            let end = rates.end_to_end_tokens_per_second.and(elapsed);
            let generation = rates
                .generation_tokens_per_second
                .and_then(|_| timing.and_then(crate::CallTiming::generation_us));
            summary.add(
                if call.error.is_some() {
                    "failed"
                } else {
                    "succeeded"
                },
                (
                    (
                        1,
                        input,
                        i64::from(input.is_some()),
                        output,
                        i64::from(output.is_some()),
                        total,
                        i64::from(total.is_some()),
                    ),
                    (
                        elapsed,
                        i64::from(elapsed.is_some()),
                        first,
                        i64::from(first.is_some()),
                    ),
                    (
                        end.and(output),
                        end,
                        i64::from(end.is_some()),
                        generation.and(output),
                        generation,
                        i64::from(generation.is_some()),
                    ),
                ),
            )?;
        }
        Ok(summary)
    }

    fn empty() -> Self {
        Self {
            attempted_calls: 0,
            statuses: ["running", "succeeded", "failed", "cancelled", "interrupted"]
                .into_iter()
                .map(|state| (state.into(), 0))
                .collect(),
            input_tokens: TokenCounter::default(),
            output_tokens: TokenCounter::default(),
            total_tokens: TokenCounter::default(),
            performance: PerformanceSummary::default(),
        }
    }
    fn add(&mut self, state: &str, row: Measures) -> Result<(), ChatError> {
        let (row, latency, rates) = row;
        let add = |left: i64, right: i64| {
            left.checked_add(right).context(ArithmeticSnafu {
                operation: "sum usage counters",
            })
        };
        self.attempted_calls = add(self.attempted_calls, row.0)?;
        let count = self.statuses.entry(state.into()).or_default();
        *count = add(*count, row.0)?;
        for (counter, known, reported) in [
            (&mut self.input_tokens, row.1, row.2),
            (&mut self.output_tokens, row.3, row.4),
            (&mut self.total_tokens, row.5, row.6),
        ] {
            counter.reported_calls = add(counter.reported_calls, reported)?;
            if let Some(known) = known {
                counter.known_subtotal = Some(add(counter.known_subtotal.unwrap_or(0), known)?);
            }
            counter.missing_calls = self
                .attempted_calls
                .checked_sub(counter.reported_calls)
                .context(ArithmeticSnafu {
                    operation: "count missing usage observations",
                })?;
        }
        self.performance
            .elapsed
            .add(latency.0, latency.1, self.attempted_calls)?;
        self.performance
            .first_token
            .add(latency.2, latency.3, self.attempted_calls)?;
        self.performance
            .end_to_end
            .add(rates.0, rates.1, rates.2, self.attempted_calls)?;
        self.performance
            .generation
            .add(rates.3, rates.4, rates.5, self.attempted_calls)?;
        Ok(())
    }
}

fn key_expression(grouping: UsageGrouping) -> &'static str {
    match grouping {
        UsageGrouping::Session => "session_id",
        UsageGrouping::Model => "model",
        UsageGrouping::Profile => "profile",
        UsageGrouping::Purpose => "purpose",
        UsageGrouping::Day => "strftime('%Y-%m-%d', started_at / 1000.0, 'unixepoch')",
    }
}

pub fn query(input: UsageInputs) -> Result<UsageOutputs, ChatError> {
    session::validate_page(input.query.limit)?;
    if let (Some(start), Some(end)) = (input.query.start_ms, input.query.end_ms) {
        ensure!(
            start <= end,
            InvalidSnafu {
                field: "time range",
                reason: "start must not exceed end"
            }
        );
    }
    ensure!(
        input.query.group_by.is_some() || input.query.after.is_none(),
        InvalidSnafu {
            field: "after",
            reason: "requires grouped statistics"
        }
    );
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    connection.transaction(|connection| {
        if let Some(id) = &input.query.session_id {
            session::load(connection, id)?;
        }
        let unreported_invocations = filtered!(
            invocations::table.select(invocations::id),
            &input.query,
            invocations
        )
        .filter(invocations::model.is_null())
        .filter(invocations::usages.is_null())
        .count()
        .get_result::<i64>(connection)
        .context(DatabaseSnafu {
            operation: "count unreported workflow invocations",
        })?;
        let totals: Vec<(String, Measures)> = filtered!(
            model_calls::table
                .select(model_calls::state)
                .group_by(model_calls::state)
                .select((model_calls::state, measures!())),
            &input.query
        )
        .load(connection)
        .context(DatabaseSnafu {
            operation: "aggregate overall model usage",
        })?;
        let mut overall = UsageSummary::empty();
        for (state, row) in totals {
            overall.add(&state, row)?;
        }
        let Some(grouping) = input.query.group_by else {
            return Ok(UsageOutputs {
                statistics: UsageStatistics {
                    unreported_invocations,
                    overall,
                    groups: vec![],
                    next_after: None,
                },
            });
        };
        // Only fixed application expressions enter SQL; caller filters and cursors remain bound.
        let key = sql::<Text>(key_expression(grouping));
        let mut keys_query = filtered!(
            model_calls::table.select(key.clone()).group_by(key.clone()),
            &input.query
        );
        if let Some(after) = &input.query.after {
            keys_query = keys_query.filter(key.clone().gt(after));
        }
        let mut keys: Vec<String> = keys_query
            .order(key.clone())
            .limit((input.query.limit + 1) as i64)
            .load(connection)
            .context(DatabaseSnafu {
                operation: "page usage groups",
            })?;
        let next_after = if keys.len() > input.query.limit {
            keys.truncate(input.query.limit);
            keys.last().cloned()
        } else {
            None
        };
        let state = sql::<Text>("state");
        let rows: Vec<(String, String, Measures)> = filtered!(
            model_calls::table
                .select((key.clone(), state.clone()))
                .group_by((key.clone(), state.clone()))
                .select((key.clone(), state, measures!())),
            &input.query
        )
        .filter(key.eq_any(&keys))
        .load(connection)
        .context(DatabaseSnafu {
            operation: "aggregate paged usage groups",
        })?;
        let mut summaries: BTreeMap<String, UsageSummary> = keys
            .into_iter()
            .map(|key| (key, UsageSummary::empty()))
            .collect();
        for (key, state, row) in rows {
            summaries
                .get_mut(&key)
                .context(InvalidSnafu {
                    field: "usage group",
                    reason: "must belong to selected keys",
                })?
                .add(&state, row)?;
        }
        Ok(UsageOutputs {
            statistics: UsageStatistics {
                unreported_invocations,
                overall,
                next_after,
                groups: summaries
                    .into_iter()
                    .map(|(key, summary)| UsageGroup { key, summary })
                    .collect(),
            },
        })
    })
}

register_operation!(
    "mf_chat.usage.query",
    QueryUsage,
    UsageInputs,
    UsageOutputs,
    query
);

impl ChatDatabase {
    pub fn usage(&self, query: UsageQuery) -> Result<UsageStatistics, ChatError> {
        Ok(self::query(UsageInputs {
            database_path: self
                .path()
                .to_str()
                .context(PathSnafu { path: self.path() })?
                .into(),
            query,
        })?
        .statistics)
    }
}

impl crate::ChatHost {
    pub fn usage(&self, query: UsageQuery) -> Result<UsageStatistics, ChatError> {
        self.database().usage(query)
    }
}
