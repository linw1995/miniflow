DROP VIEW usage_facts;
ALTER TABLE model_calls RENAME TO previous_model_calls;
CREATE TABLE model_calls (
    id TEXT NOT NULL PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    turn_id TEXT,
    purpose TEXT NOT NULL CHECK (purpose IN ('conversation', 'title')),
    profile TEXT NOT NULL,
    model TEXT NOT NULL,
    config_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'succeeded', 'failed', 'cancelled', 'interrupted', 'skipped')),
    title_revision BIGINT,
    started_at BIGINT,
    finished_at BIGINT,
    input_tokens BIGINT CHECK (input_tokens >= 0),
    output_tokens BIGINT CHECK (output_tokens >= 0),
    total_tokens BIGINT CHECK (total_tokens >= 0),
    finish_reason TEXT,
    error TEXT,
    result_text TEXT,
    disposition TEXT,
    FOREIGN KEY (turn_id, session_id) REFERENCES turns(id, session_id),
    CHECK (purpose <> 'conversation' OR turn_id IS NOT NULL)
);
INSERT INTO model_calls (id, session_id, turn_id, purpose, profile, model, config_id, state, title_revision, started_at, finished_at, input_tokens, output_tokens, total_tokens, finish_reason, error, result_text, disposition) SELECT id, session_id, turn_id, purpose, profile, COALESCE(model, ''), config_id, state, title_revision, started_at, finished_at, input_tokens, output_tokens, total_tokens, finish_reason, error, result_text, disposition FROM previous_model_calls;
DROP TABLE previous_model_calls;
CREATE UNIQUE INDEX one_conversation_call_per_turn ON model_calls(turn_id) WHERE purpose = 'conversation';
CREATE UNIQUE INDEX one_active_title_per_session ON model_calls(session_id)
    WHERE purpose = 'title' AND state IN ('pending', 'running');
CREATE INDEX call_filters ON model_calls(session_id, purpose, config_id, started_at);
CREATE INDEX pending_title_calls ON model_calls(state, purpose, id);
ALTER TABLE messages DROP COLUMN metadata;
