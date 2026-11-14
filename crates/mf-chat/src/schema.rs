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
    }
}

diesel::table! {
    messages (id) {
        id -> Text,
        session_id -> Text,
        turn_id -> Text,
        sequence -> BigInt,
        role -> Text,
        content -> Text,
        created_at -> BigInt,
        metadata -> Nullable<Text>,
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
    }
}

diesel::joinable!(turns -> sessions (session_id));
diesel::joinable!(messages -> sessions (session_id));
diesel::joinable!(messages -> turns (turn_id));
diesel::joinable!(model_calls -> sessions (session_id));
diesel::joinable!(model_calls -> turns (turn_id));
diesel::table! {
    tool_use_sources (call_id) {
        call_id -> Text,
        incomplete -> Bool,
        finished -> Bool,
    }
}
diesel::table! {
    tool_executions (call_id, id) {
        call_id -> Text,
        id -> Text,
        ordinal -> BigInt,
        name -> Text,
        phase -> Text,
        arguments -> Text,
        output -> Text,
        duration_ns -> Nullable<BigInt>,
        model_call_id -> Nullable<Text>,
        provider_tool_call_id -> Nullable<Text>,
        semantic_call_id -> Nullable<Text>,
    }
}
diesel::table! {
    message_streams (call_id) {
        call_id -> Text,
        finished -> Bool,
    }
}
diesel::table! {
    message_sources (message_id) {
        message_id -> Text,
        call_id -> Text,
        ordinal -> BigInt,
        model_call_id -> Nullable<Text>,
        tool_calls_present -> Bool,
        metadata_secrets -> Nullable<Text>,
    }
}
diesel::table! {
    chat_tool_calls (id) {
        id -> Text,
        call_id -> Text,
        assistant_message_id -> Text,
        ordinal -> BigInt,
        provider_tool_call_id -> Text,
        name -> Text,
        arguments -> Text,
        argument_secrets -> Nullable<Text>,
        public_arguments -> Text,
        result_message_id -> Nullable<Text>,
    }
}
diesel::joinable!(tool_use_sources -> model_calls (call_id));
diesel::joinable!(tool_executions -> tool_use_sources (call_id));
diesel::joinable!(message_streams -> model_calls (call_id));
diesel::joinable!(message_sources -> message_streams (call_id));
diesel::joinable!(message_sources -> messages (message_id));
diesel::joinable!(chat_tool_calls -> model_calls (call_id));
diesel::allow_tables_to_appear_in_same_query!(
    sessions,
    turns,
    messages,
    model_calls,
    tool_use_sources,
    tool_executions,
    message_streams,
    message_sources,
    chat_tool_calls
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
