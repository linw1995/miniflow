ALTER TABLE sessions ADD COLUMN conversation_revision BIGINT NOT NULL DEFAULT 0 CHECK (conversation_revision >= 0);
UPDATE sessions SET conversation_revision = (
    SELECT COUNT(*) FROM turns WHERE turns.session_id = sessions.id AND turns.state = 'succeeded'
);
-- Older usage-list schemas can read the migrated call facts without a lossy reversal.
