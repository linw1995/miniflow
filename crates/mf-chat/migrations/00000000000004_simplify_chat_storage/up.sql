ALTER TABLE sessions DROP COLUMN conversation_revision;

-- Preserve reported legacy counters as one stable call fact without changing existing lists.
UPDATE model_calls
SET usages = json_array(json_object(
    'call_id', id,
    'model', model,
    'started_at', started_at,
    'usage', json_patch(json_patch(
        CASE WHEN input_tokens IS NULL THEN '{}' ELSE json_object('input_tokens', input_tokens) END,
        CASE WHEN output_tokens IS NULL THEN '{}' ELSE json_object('output_tokens', output_tokens) END),
        CASE WHEN total_tokens IS NULL THEN '{}' ELSE json_object('total_tokens', total_tokens) END)
))
WHERE usages IS NULL AND model IS NOT NULL AND state = 'succeeded'
    AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL OR total_tokens IS NOT NULL);
