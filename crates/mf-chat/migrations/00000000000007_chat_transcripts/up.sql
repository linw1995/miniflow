ALTER TABLE sessions ADD COLUMN context_revision BIGINT NOT NULL DEFAULT 0 CHECK (context_revision >= 0);
ALTER TABLE turns ADD COLUMN context_revision BIGINT NOT NULL DEFAULT 0 CHECK (context_revision >= 0);
ALTER TABLE turns ADD COLUMN result_alternatives TEXT CHECK (result_alternatives IS NULL OR result_alternatives IN ('"append"', '"replace_context"', '"both"'));
ALTER TABLE model_calls ADD COLUMN terminal_result TEXT CHECK (terminal_result IS NULL OR (json_valid(terminal_result) AND json_type(terminal_result) = 'object'));

CREATE TABLE transcript_messages (
    id TEXT NOT NULL PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    turn_id TEXT NOT NULL,
    sequence BIGINT NOT NULL CHECK (sequence >= 0),
    payload TEXT NOT NULL CHECK (json_valid(payload) AND json_type(payload) = 'object'),
    created_at BIGINT NOT NULL,
    FOREIGN KEY (turn_id, session_id) REFERENCES turns(id, session_id),
    UNIQUE (session_id, sequence)
);
INSERT INTO transcript_messages (id, session_id, turn_id, sequence, payload, created_at)
SELECT id, session_id, turn_id, sequence,
       CASE WHEN metadata IS NULL THEN json_object('role', role, 'content', content)
            ELSE json_object('role', role, 'content', content, 'metadata', json(metadata)) END,
       created_at
FROM messages;
DROP TABLE messages;
ALTER TABLE transcript_messages RENAME TO messages;
CREATE INDEX message_order ON messages(session_id, sequence);
CREATE INDEX turn_messages ON messages(turn_id, sequence);

CREATE TABLE context_revisions (
    session_id TEXT NOT NULL REFERENCES sessions(id),
    revision BIGINT NOT NULL CHECK (revision >= 0),
    parent_revision BIGINT CHECK (parent_revision >= 0 AND parent_revision < revision),
    turn_id TEXT UNIQUE,
    operation TEXT NOT NULL CHECK (operation IN ('checkpoint', 'append')),
    snapshot TEXT CHECK (snapshot IS NULL OR (json_valid(snapshot) AND json_type(snapshot) = 'array')),
    PRIMARY KEY (session_id, revision),
    FOREIGN KEY (session_id, parent_revision) REFERENCES context_revisions(session_id, revision),
    FOREIGN KEY (turn_id, session_id) REFERENCES turns(id, session_id),
    CHECK ((operation = 'append' AND snapshot IS NULL AND parent_revision IS NOT NULL AND turn_id IS NOT NULL)
        OR (operation = 'checkpoint' AND snapshot IS NOT NULL))
);
INSERT INTO context_revisions (session_id, revision, parent_revision, turn_id, operation, snapshot)
SELECT id, 0, NULL, NULL, 'checkpoint',
       CASE WHEN system IS NULL OR length(system) = 0 THEN '[]'
            ELSE json_array(json_object('role', 'system', 'content', system)) END
FROM sessions;
INSERT INTO context_revisions (session_id, revision, parent_revision, turn_id, operation, snapshot)
SELECT session_id, revision, revision - 1, id, 'append', NULL
FROM (
    SELECT id, session_id, row_number() OVER (PARTITION BY session_id ORDER BY sequence) AS revision
    FROM turns WHERE state = 'succeeded'
);
UPDATE turns SET context_revision = (
    SELECT count(*) FROM turns prior
    WHERE prior.session_id = turns.session_id AND prior.state = 'succeeded' AND prior.sequence < turns.sequence
);
UPDATE sessions SET context_revision = (
    SELECT count(*) FROM turns WHERE turns.session_id = sessions.id AND turns.state = 'succeeded'
);

-- Only known complete legacy results can be replayed; unknown attempts stay unknown.
UPDATE model_calls SET terminal_result = json_object('append',
    json_patch(json_object(
        'status', 'succeeded',
        'messages', json_array(json((SELECT payload FROM messages
            WHERE messages.turn_id = model_calls.turn_id AND json_extract(payload, '$.role') = 'assistant'))),
        'usages', json(usages)),
        CASE WHEN result_title IS NULL THEN '{}' ELSE json_object('title', result_title) END))
WHERE purpose = 'conversation' AND state = 'succeeded' AND usages IS NOT NULL AND result_text IS NOT NULL
    AND EXISTS (SELECT 1 FROM messages WHERE messages.turn_id = model_calls.turn_id
        AND json_extract(payload, '$.role') = 'assistant');
