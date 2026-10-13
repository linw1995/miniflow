CREATE TABLE sessions (
    id TEXT NOT NULL PRIMARY KEY,
    title TEXT NOT NULL,
    title_source TEXT NOT NULL CHECK (title_source IN ('provisional', 'generated', 'manual')),
    title_revision BIGINT NOT NULL CHECK (title_revision >= 0),
    conversation_revision BIGINT NOT NULL CHECK (conversation_revision >= 0),
    profile TEXT NOT NULL,
    system TEXT,
    archived BOOLEAN NOT NULL CHECK (archived IN (0, 1)),
    next_sequence BIGINT NOT NULL CHECK (next_sequence >= 0),
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE turns (
    id TEXT NOT NULL PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    retry_of TEXT REFERENCES turns(id),
    sequence BIGINT NOT NULL CHECK (sequence >= 0),
    state TEXT NOT NULL CHECK (state IN ('running', 'succeeded', 'failed', 'cancelled', 'interrupted')),
    created_at BIGINT NOT NULL,
    finished_at BIGINT,
    error TEXT,
    UNIQUE (session_id, sequence),
    UNIQUE (id, session_id)
);

CREATE TABLE messages (
    id TEXT NOT NULL PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    turn_id TEXT NOT NULL,
    sequence BIGINT NOT NULL CHECK (sequence >= 0),
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content TEXT NOT NULL CHECK (length(content) > 0),
    created_at BIGINT NOT NULL,
    FOREIGN KEY (turn_id, session_id) REFERENCES turns(id, session_id),
    UNIQUE (session_id, sequence),
    UNIQUE (turn_id, role)
);

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

CREATE UNIQUE INDEX one_running_turn_per_session ON turns(session_id) WHERE state = 'running';
CREATE UNIQUE INDEX one_conversation_call_per_turn ON model_calls(turn_id) WHERE purpose = 'conversation';
CREATE UNIQUE INDEX one_active_title_per_session ON model_calls(session_id)
    WHERE purpose = 'title' AND state IN ('pending', 'running');
CREATE INDEX session_listing ON sessions(archived, created_at, id);
CREATE INDEX message_order ON messages(session_id, sequence);
CREATE INDEX call_filters ON model_calls(session_id, purpose, config_id, started_at);
CREATE INDEX pending_title_calls ON model_calls(state, purpose, id);
