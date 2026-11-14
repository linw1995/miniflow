-- A rich message stream cannot be represented by the previous two-message schema.
CREATE TEMP TABLE downgrade_message_guard (valid INTEGER NOT NULL CHECK (valid = 1));
INSERT INTO downgrade_message_guard SELECT CASE WHEN EXISTS (SELECT 1 FROM message_streams) THEN 0 ELSE 1 END;
DROP TABLE downgrade_message_guard;

ALTER TABLE model_calls ADD COLUMN tool_uses TEXT;
UPDATE model_calls SET tool_uses = (
    SELECT json_object(
        'invocation', json_object('session_id', model_calls.session_id, 'turn_id', model_calls.turn_id, 'call_id', model_calls.id),
        'incomplete', json(CASE WHEN s.incomplete THEN 'true' ELSE 'false' END),
        'finished', json(CASE WHEN s.finished THEN 'true' ELSE 'false' END),
        'runs', json(COALESCE((SELECT json_group_array(json_object(
            'id', e.id, 'name', e.name, 'phase', e.phase, 'arguments', e.arguments,
            'output', e.output, 'duration_ns', e.duration_ns
        )) FROM (SELECT * FROM tool_executions WHERE call_id = s.call_id ORDER BY ordinal) e), '[]'))
    ) FROM tool_use_sources s WHERE s.call_id = model_calls.id
) WHERE EXISTS (SELECT 1 FROM tool_use_sources s WHERE s.call_id = model_calls.id);
DROP TABLE chat_tool_calls;
DROP TABLE message_sources;
DROP TABLE message_streams;
DROP TABLE tool_executions;
DROP TABLE tool_use_sources;

ALTER TABLE messages RENAME TO expanded_messages;
CREATE TABLE messages (
    id TEXT NOT NULL PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    turn_id TEXT NOT NULL,
    sequence BIGINT NOT NULL CHECK (sequence >= 0),
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content TEXT NOT NULL CHECK (length(content) > 0),
    created_at BIGINT NOT NULL,
    metadata TEXT CHECK (metadata IS NULL OR (json_valid(metadata) AND json_type(metadata) = 'object')),
    FOREIGN KEY (turn_id, session_id) REFERENCES turns(id, session_id),
    UNIQUE (session_id, sequence),
    UNIQUE (turn_id, role)
);
INSERT INTO messages SELECT * FROM expanded_messages;
DROP TABLE expanded_messages;
CREATE INDEX message_order ON messages(session_id, sequence);
