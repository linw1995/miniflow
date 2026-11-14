CREATE TABLE tool_use_sources (
    call_id TEXT NOT NULL PRIMARY KEY REFERENCES model_calls(id),
    incomplete BOOLEAN NOT NULL CHECK (incomplete IN (0, 1)),
    finished BOOLEAN NOT NULL CHECK (finished IN (0, 1))
);

CREATE TABLE tool_executions (
    call_id TEXT NOT NULL REFERENCES tool_use_sources(call_id),
    id TEXT NOT NULL,
    ordinal BIGINT NOT NULL CHECK (ordinal >= 0),
    name TEXT NOT NULL,
    phase TEXT NOT NULL CHECK (phase IN ('preparing', 'running', 'succeeded', 'failed', 'skipped', 'interrupted', 'unknown')),
    arguments TEXT NOT NULL,
    output TEXT NOT NULL,
    duration_ns BIGINT CHECK (duration_ns >= 0),
    model_call_id TEXT,
    provider_tool_call_id TEXT,
    semantic_call_id TEXT,
    FOREIGN KEY (semantic_call_id, call_id) REFERENCES chat_tool_calls(id, call_id),
    PRIMARY KEY (call_id, id),
    UNIQUE (call_id, ordinal)
);

ALTER TABLE messages RENAME TO previous_messages;
CREATE TABLE messages (
    id TEXT NOT NULL PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    turn_id TEXT NOT NULL,
    sequence BIGINT NOT NULL CHECK (sequence >= 0),
    role TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant', 'tool')),
    content TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    metadata TEXT CHECK (metadata IS NULL OR (json_valid(metadata) AND json_type(metadata) = 'object')),
    FOREIGN KEY (turn_id, session_id) REFERENCES turns(id, session_id),
    UNIQUE (session_id, sequence)
);
INSERT INTO messages SELECT * FROM previous_messages;
DROP TABLE previous_messages;
CREATE INDEX message_order ON messages(session_id, sequence);

CREATE TABLE message_streams (
    call_id TEXT NOT NULL PRIMARY KEY REFERENCES model_calls(id),
    finished BOOLEAN NOT NULL CHECK (finished IN (0, 1))
);
CREATE TABLE message_sources (
    message_id TEXT NOT NULL PRIMARY KEY REFERENCES messages(id),
    call_id TEXT NOT NULL REFERENCES message_streams(call_id),
    ordinal BIGINT NOT NULL CHECK (ordinal >= 0),
    model_call_id TEXT,
    tool_calls_present BOOLEAN NOT NULL CHECK (tool_calls_present IN (0, 1)),
    metadata_secrets TEXT CHECK (metadata_secrets IS NULL OR json_valid(metadata_secrets)),
    UNIQUE (call_id, ordinal),
    UNIQUE (message_id, call_id)
);
CREATE TABLE chat_tool_calls (
    id TEXT NOT NULL PRIMARY KEY,
    call_id TEXT NOT NULL REFERENCES model_calls(id),
    assistant_message_id TEXT NOT NULL REFERENCES messages(id),
    ordinal BIGINT NOT NULL CHECK (ordinal >= 0),
    provider_tool_call_id TEXT NOT NULL,
    name TEXT NOT NULL,
    arguments TEXT NOT NULL CHECK (json_valid(arguments)),
    argument_secrets TEXT CHECK (argument_secrets IS NULL OR json_valid(argument_secrets)),
    public_arguments TEXT NOT NULL,
    result_message_id TEXT REFERENCES messages(id),
    FOREIGN KEY (assistant_message_id, call_id) REFERENCES message_sources(message_id, call_id),
    FOREIGN KEY (result_message_id, call_id) REFERENCES message_sources(message_id, call_id),
    UNIQUE (assistant_message_id, provider_tool_call_id),
    UNIQUE (assistant_message_id, ordinal),
    UNIQUE (result_message_id),
    UNIQUE (id, call_id)
);
CREATE INDEX tool_execution_origin ON tool_executions(call_id, model_call_id, provider_tool_call_id);
CREATE INDEX chat_tool_call_owner ON chat_tool_calls(call_id, assistant_message_id);

-- Earlier draft journals contain observations, not canonical messages or invented associations.
INSERT INTO tool_use_sources (call_id, incomplete, finished)
SELECT id, json_extract(tool_uses, '$.incomplete'), json_extract(tool_uses, '$.finished')
FROM model_calls WHERE tool_uses IS NOT NULL;
INSERT INTO tool_executions (call_id, id, ordinal, name, phase, arguments, output, duration_ns)
SELECT c.id, json_extract(t.value, '$.id'), CAST(t.key AS INTEGER),
       json_extract(t.value, '$.name'), json_extract(t.value, '$.phase'),
       json_extract(t.value, '$.arguments'), json_extract(t.value, '$.output'),
       json_extract(t.value, '$.duration_ns')
FROM model_calls c, json_each(c.tool_uses, '$.runs') t WHERE c.tool_uses IS NOT NULL;
ALTER TABLE model_calls DROP COLUMN tool_uses;
