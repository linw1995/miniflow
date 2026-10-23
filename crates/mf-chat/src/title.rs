use crate::{
    ChatError, ModelCall,
    error::*,
    schema::{model_calls, sessions},
    session,
};
use diesel::prelude::*;
use snafu::{OptionExt, ResultExt};

#[cfg(test)]
mod tests;

pub fn attempted(connection: &mut SqliteConnection, session_id: &str) -> Result<bool, ChatError> {
    diesel::select(diesel::dsl::exists(
        model_calls::table
            .filter(model_calls::session_id.eq(session_id))
            .filter(diesel::dsl::sql::<diesel::sql_types::Bool>(
                "((purpose = 'title' AND started_at IS NOT NULL) OR EXISTS (SELECT 1 FROM json_each(model_calls.usages) AS u WHERE json_extract(u.value, '$.attribution.purpose') = 'title'))",
            )),
    ))
    .get_result(connection)
    .context(DatabaseSnafu { operation: "load title attempt context" })
}

pub fn apply(
    connection: &mut SqliteConnection,
    call: &ModelCall,
    candidate: Option<&str>,
    timestamp: i64,
) -> Result<(), ChatError> {
    let Some(candidate) = candidate else {
        return Ok(());
    };
    let title = candidate.trim();
    // A rejected candidate must not discard the primary reply or its paid call facts.
    if title.is_empty() || title.chars().count() > 120 || title.chars().any(char::is_control) {
        return Ok(());
    }
    let owner = session::load(connection, &call.session_id)?;
    if owner.title_source != "provisional" || call.title_revision != Some(owner.title_revision) {
        return Ok(());
    }
    let revision = owner
        .title_revision
        .checked_add(1)
        .context(ArithmeticSnafu {
            operation: "advance generated title revision",
        })?;
    diesel::update(sessions::table.find(&owner.id))
        .set((
            sessions::title.eq(title),
            sessions::title_source.eq("generated"),
            sessions::title_revision.eq(revision),
            sessions::updated_at.eq(timestamp),
        ))
        .execute(connection)
        .context(DatabaseSnafu {
            operation: "apply generated session title",
        })?;
    Ok(())
}
