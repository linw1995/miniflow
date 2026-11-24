CREATE TABLE preview_segments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    turn_id TEXT NOT NULL REFERENCES turns(id) ON DELETE CASCADE,
    model_call_id TEXT NOT NULL,
    part TEXT NOT NULL,
    tool_id TEXT,
    name TEXT,
    bytes BIGINT NOT NULL CHECK (typeof(bytes) = 'integer' AND bytes >= 0),
    UNIQUE (turn_id, model_call_id, part)
);

CREATE TABLE preview_chunks (
    segment_id INTEGER NOT NULL REFERENCES preview_segments(id) ON DELETE CASCADE,
    end_byte BIGINT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (segment_id, end_byte)
) WITHOUT ROWID;
