//! Old-schema fixtures, deliberately bypassing the current messages-required workflow contract.

use crate::{ChatDatabase, ConversationResult, Invocation, Message, schema::*, storage};
use diesel::prelude::*;

pub fn complete_legacy(
    database: &ChatDatabase,
    invocation: &Invocation,
    response: ConversationResult,
) {
    let mut connection = storage::connect(database.path()).unwrap();
    connection
        .immediate_transaction::<_, diesel::result::Error, _>(|connection| {
            let owner = crate::session::load(connection, &invocation.session_id).unwrap();
            let timestamp = crate::session::now().unwrap();
            diesel::insert_into(messages::table)
                .values(Message {
                    id: uuid::Uuid::new_v4().to_string(),
                    session_id: owner.id.clone(),
                    turn_id: invocation.turn_id.clone(),
                    sequence: owner.next_sequence,
                    role: "assistant".into(),
                    content: response.text.clone(),
                    created_at: timestamp,
                    metadata: response.metadata.clone(),
                })
                .execute(connection)?;
            crate::turn::save_response(
                connection,
                &invocation.call_id,
                &response,
                timestamp,
                "committed",
            )
            .unwrap();
            diesel::update(turns::table.find(&invocation.turn_id))
                .set((
                    turns::state.eq("succeeded"),
                    turns::finished_at.eq(timestamp),
                ))
                .execute(connection)?;
            diesel::update(sessions::table.find(&owner.id))
                .set(sessions::next_sequence.eq(owner.next_sequence + 1))
                .execute(connection)?;
            Ok(())
        })
        .unwrap();
}
