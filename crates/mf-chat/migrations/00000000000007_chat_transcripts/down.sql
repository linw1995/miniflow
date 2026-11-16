-- Older schemas cannot represent tool exchanges, context rewrites, or complete failed results.
CREATE TEMP TABLE mf_chat_v7_downgrade (compatible INTEGER NOT NULL CHECK (compatible = 1));
INSERT INTO mf_chat_v7_downgrade
SELECT CASE WHEN
    EXISTS (SELECT 1 FROM messages WHERE json_extract(payload, '$.role') NOT IN ('user', 'assistant')
        OR length(json_extract(payload, '$.content')) = 0 OR json_type(payload, '$.tool_calls') IS NOT NULL)
    OR EXISTS (SELECT 1 FROM messages GROUP BY turn_id, json_extract(payload, '$.role') HAVING count(*) > 1)
    OR EXISTS (SELECT 1 FROM context_revisions WHERE operation = 'checkpoint' AND revision > 0)
    OR EXISTS (SELECT 1 FROM model_calls WHERE terminal_result IS NOT NULL
        AND json_extract(terminal_result, '$.append.status') IS NOT 'succeeded')
THEN 0 ELSE 1 END;
DROP TABLE mf_chat_v7_downgrade;
UPDATE model_calls SET
    result_text = json_extract(terminal_result, '$.append.messages[0].content'),
    metadata = CASE WHEN json_type(terminal_result, '$.append.messages[0].metadata') IS NULL THEN NULL
        ELSE json_extract(terminal_result, '$.append.messages[0].metadata') END
WHERE terminal_result IS NOT NULL;
DROP TABLE context_revisions;
CREATE TABLE legacy_messages (
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
INSERT INTO legacy_messages (id, session_id, turn_id, sequence, role, content, created_at, metadata)
SELECT id, session_id, turn_id, sequence, json_extract(payload, '$.role'),
    json_extract(payload, '$.content'), created_at, json_extract(payload, '$.metadata') FROM messages;
DROP TABLE messages;
ALTER TABLE legacy_messages RENAME TO messages;
CREATE INDEX message_order ON messages(session_id, sequence);
ALTER TABLE model_calls DROP COLUMN terminal_result;
ALTER TABLE turns DROP COLUMN result_alternatives;
ALTER TABLE turns DROP COLUMN context_revision;
ALTER TABLE sessions DROP COLUMN context_revision;
