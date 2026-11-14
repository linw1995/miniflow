-- The preceding schema cannot express history replacement. Refuse destructive downgrades.
CREATE TEMP TABLE conversation_state_downgrade_guard (empty INTEGER CHECK (empty = 0));
INSERT INTO conversation_state_downgrade_guard SELECT COUNT(*) FROM conversation_states;
DROP TABLE conversation_state_downgrade_guard;
DROP TABLE message_event_sources;
DROP TABLE conversation_state_entries;
DROP TABLE conversation_payload_tools;
DROP TABLE conversation_payloads;
DROP TABLE conversation_states;
DROP INDEX conversation_call_session;
