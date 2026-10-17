use crate::{
    CallPurpose, ChatDatabase, ChatError,
    error::*,
    flows,
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

#[diesel::declare_sql_function]
extern "SQL" {
    #[aggregate]
    #[sql_name = "SUM"]
    fn sum_i64(value: Nullable<BigInt>) -> Nullable<BigInt>;
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct UsageSummary {
    pub attempted_calls: i64,
    pub statuses: BTreeMap<String, i64>,
    pub input_tokens: TokenCounter,
    pub output_tokens: TokenCounter,
    pub total_tokens: TokenCounter,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct UsageGroup {
    pub key: String,
    pub summary: UsageSummary,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
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

type Measures = (i64, Option<i64>, i64, Option<i64>, i64, Option<i64>, i64);

macro_rules! measures {
    () => {
        (
            count_star(),
            sum_i64(model_calls::input_tokens),
            count(model_calls::input_tokens),
            sum_i64(model_calls::output_tokens),
            count(model_calls::output_tokens),
            sum_i64(model_calls::total_tokens),
            count(model_calls::total_tokens),
        )
    };
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
        }
    }
    fn add(&mut self, state: &str, row: Measures) -> Result<(), ChatError> {
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
        let registry = mf_runtime::NodeRegistry::from_inventory()?;
        let flow = flows::usage_statistics::prepare_workflow(&registry, None)?;
        let input = UsageInputs {
            database_path: self
                .path()
                .to_str()
                .context(PathSnafu { path: self.path() })?
                .into(),
            query,
        };
        let arguments = mf_runtime::WorkflowArguments(
            input
                .into_values()?
                .into_iter()
                .map(|(name, value)| (format!("query.{name}"), value))
                .collect(),
        );
        let outputs = mf_runtime::FlowRuntime::default().execute_with_inputs(&flow, arguments)?;
        Ok(UsageOutputs::from_values(outputs)?.statistics)
    }
}

impl crate::ChatHost {
    pub fn usage(&self, query: UsageQuery) -> Result<UsageStatistics, ChatError> {
        self.database().usage(query)
    }
}
