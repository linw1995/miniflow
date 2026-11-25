CREATE TABLE steering_deliveries (
    turn_id TEXT NOT NULL REFERENCES turns(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    content TEXT NOT NULL,
    PRIMARY KEY (turn_id, position)
);
