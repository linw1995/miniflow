// Kept in sync with the bundled SQLite migration.

diesel::table! {
    records (commit_sha) {
        commit_sha -> Text,
        run_id -> Text,
        attempt -> BigInt,
        schema_version -> BigInt,
        signature -> Text,
        created_at -> BigInt,
    }
}

diesel::table! {
    tests (id) {
        id -> BigInt,
        package -> Text,
        binary -> Text,
        name -> Text,
    }
}

diesel::table! {
    files (id) {
        id -> BigInt,
        path -> Text,
    }
}

// The coverage table has no SQL primary key; Diesel requires declaring one,
// so the macro names the full row. Nothing queries coverage by key.
diesel::table! {
    coverage (commit_sha, file_id, line_start, line_end, test_id) {
        commit_sha -> Text,
        file_id -> BigInt,
        line_start -> BigInt,
        line_end -> BigInt,
        test_id -> BigInt,
    }
}

diesel::joinable!(coverage -> records (commit_sha));
diesel::joinable!(coverage -> files (file_id));
diesel::joinable!(coverage -> tests (test_id));
diesel::allow_tables_to_appear_in_same_query!(records, tests, files, coverage);
