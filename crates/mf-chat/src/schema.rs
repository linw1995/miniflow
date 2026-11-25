// @generated from the bundled SQLite migration.

diesel::table! {
    sessions (id) {
        id -> Text,
        title -> Text,
        title_source -> Text,
        title_revision -> BigInt,
        profile -> Text,
        system -> Nullable<Text>,
        archived -> Bool,
        next_sequence -> BigInt,
        created_at -> BigInt,
        updated_at -> BigInt,
        context_revision -> BigInt,
    }
}

diesel::table! {
    turns (id) {
        id -> Text,
        session_id -> Text,
        retry_of -> Nullable<Text>,
        sequence -> BigInt,
        state -> Text,
        created_at -> BigInt,
        finished_at -> Nullable<BigInt>,
        error -> Nullable<Text>,
        context_revision -> BigInt,
        result_alternatives -> Nullable<Text>,
    }
}

diesel::table! {
    messages (id) {
        id -> Text,
        session_id -> Text,
        turn_id -> Text,
        sequence -> BigInt,
        payload -> Text,
        created_at -> BigInt,
    }
}

diesel::table! {
    model_calls (id) {
        id -> Text,
        session_id -> Text,
        turn_id -> Nullable<Text>,
        purpose -> Text,
        profile -> Text,
        model -> Nullable<Text>,
        state -> Text,
        title_revision -> Nullable<BigInt>,
        started_at -> Nullable<BigInt>,
        finished_at -> Nullable<BigInt>,
        input_tokens -> Nullable<BigInt>,
        output_tokens -> Nullable<BigInt>,
        total_tokens -> Nullable<BigInt>,
        finish_reason -> Nullable<Text>,
        error -> Nullable<Text>,
        result_text -> Nullable<Text>,
        disposition -> Nullable<Text>,
        usages -> Nullable<Text>,
        metadata -> Nullable<Text>,
        result_title -> Nullable<Text>,
        terminal_result -> Nullable<Text>,
    }
}

diesel::table! {
    context_revisions (session_id, revision) {
        session_id -> Text,
        revision -> BigInt,
        parent_revision -> Nullable<BigInt>,
        turn_id -> Nullable<Text>,
        operation -> Text,
        snapshot -> Nullable<Text>,
    }
}

diesel::table! {
    steering_deliveries (turn_id, position) {
        turn_id -> Text,
        position -> BigInt,
        content -> Text,
    }
}

diesel::joinable!(turns -> sessions (session_id));
diesel::joinable!(messages -> sessions (session_id));
diesel::joinable!(messages -> turns (turn_id));
diesel::joinable!(model_calls -> sessions (session_id));
diesel::joinable!(model_calls -> turns (turn_id));
diesel::allow_tables_to_appear_in_same_query!(
    sessions,
    turns,
    messages,
    model_calls,
    context_revisions
);

diesel::table! {
    usage_facts (id) {
        id -> Text,
        session_id -> Text,
        purpose -> Text,
        profile -> Text,
        model -> Text,
        state -> Text,
        started_at -> Nullable<BigInt>,
        input_tokens -> Nullable<BigInt>,
        output_tokens -> Nullable<BigInt>,
        total_tokens -> Nullable<BigInt>,
        elapsed_us -> Nullable<BigInt>,
        first_token_us -> Nullable<BigInt>,
    }
}
