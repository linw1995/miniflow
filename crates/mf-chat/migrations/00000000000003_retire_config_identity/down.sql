DROP VIEW usage_facts;
DROP INDEX call_filters;
CREATE INDEX call_filters ON model_calls(session_id, purpose, config_id, started_at);
CREATE VIEW usage_facts AS
SELECT c.id || ':' || u.key AS id, c.session_id, c.purpose, c.profile,
       json_extract(u.value, '$.model') AS model, c.config_id,
       CASE WHEN json_extract(u.value, '$.error') IS NULL THEN 'succeeded' ELSE 'failed' END AS state,
       COALESCE(json_extract(u.value, '$.started_at'), c.started_at) AS started_at,
       json_extract(u.value, '$.usage.input_tokens') AS input_tokens,
       json_extract(u.value, '$.usage.output_tokens') AS output_tokens,
       json_extract(u.value, '$.usage.total_tokens') AS total_tokens
FROM model_calls c, json_each(c.usages) u
WHERE c.usages IS NOT NULL
UNION ALL
SELECT id, session_id, purpose, profile, model, config_id, state, started_at,
       input_tokens, output_tokens, total_tokens
FROM model_calls WHERE usages IS NULL AND model IS NOT NULL;
