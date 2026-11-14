-- Canonical context versions are independent of the append-only message evidence journal.
-- Payload sharing is storage interning only: message identity is (call_id, ordinal).
CREATE UNIQUE INDEX conversation_call_session ON model_calls(id, session_id);
CREATE TABLE conversation_states (
    call_id TEXT NOT NULL PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    finished BOOLEAN NOT NULL CHECK (finished IN (0, 1)),
    FOREIGN KEY (call_id, session_id) REFERENCES model_calls(id, session_id),
    UNIQUE (call_id, session_id)
);
CREATE TABLE conversation_payloads (
    session_id TEXT NOT NULL REFERENCES sessions(id),
    id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant', 'tool')),
    content TEXT NOT NULL,
    metadata TEXT CHECK (metadata IS NULL OR (json_valid(metadata) AND json_type(metadata) = 'object')),
    metadata_secrets TEXT CHECK (metadata_secrets IS NULL OR json_valid(metadata_secrets)),
    tool_calls_present BOOLEAN NOT NULL CHECK (tool_calls_present IN (0, 1)),
    tool_call_id TEXT,
    PRIMARY KEY (session_id, id)
);
CREATE TABLE conversation_payload_tools (
    session_id TEXT NOT NULL,
    payload_id TEXT NOT NULL,
    ordinal BIGINT NOT NULL CHECK (ordinal >= 0),
    provider_tool_call_id TEXT NOT NULL,
    name TEXT NOT NULL,
    arguments TEXT NOT NULL CHECK (json_valid(arguments)),
    argument_secrets TEXT CHECK (argument_secrets IS NULL OR json_valid(argument_secrets)),
    public_arguments TEXT NOT NULL,
    PRIMARY KEY (session_id, payload_id, ordinal),
    FOREIGN KEY (session_id, payload_id) REFERENCES conversation_payloads(session_id, id),
    UNIQUE (session_id, payload_id, provider_tool_call_id)
);
CREATE TABLE conversation_state_entries (
    call_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    ordinal BIGINT NOT NULL CHECK (ordinal >= 0),
    payload_id TEXT NOT NULL,
    PRIMARY KEY (call_id, ordinal),
    FOREIGN KEY (call_id, session_id) REFERENCES conversation_states(call_id, session_id),
    FOREIGN KEY (session_id, payload_id) REFERENCES conversation_payloads(session_id, id)
);
CREATE INDEX conversation_payload_members ON conversation_state_entries(session_id, payload_id);
CREATE TABLE message_event_sources (
    call_id TEXT NOT NULL REFERENCES model_calls(id),
    id TEXT NOT NULL,
    offset BIGINT NOT NULL CHECK (offset >= 0),
    count BIGINT NOT NULL CHECK (count > 0),
    PRIMARY KEY (call_id, id),
    FOREIGN KEY (call_id, offset) REFERENCES message_sources(call_id, ordinal),
    UNIQUE (call_id, offset)
);
