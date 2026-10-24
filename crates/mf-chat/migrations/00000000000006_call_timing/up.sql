DROP VIEW usage_facts;
CREATE VIEW usage_facts AS
SELECT c.id || ':' || u.key AS id, c.session_id,
       COALESCE(json_extract(u.value, '$.attribution.purpose'), c.purpose) AS purpose,
       COALESCE(json_extract(u.value, '$.attribution.profile'), c.profile) AS profile,
       json_extract(u.value, '$.model') AS model,
       CASE WHEN json_extract(u.value, '$.error') IS NULL THEN 'succeeded' ELSE 'failed' END AS state,
       COALESCE(json_extract(u.value, '$.started_at'), c.started_at) AS started_at,
       json_extract(u.value, '$.usage.input_tokens') AS input_tokens,
       json_extract(u.value, '$.usage.output_tokens') AS output_tokens,
       json_extract(u.value, '$.usage.total_tokens') AS total_tokens,
       json_extract(u.value, '$.timing.elapsed_us') AS elapsed_us,
       json_extract(u.value, '$.timing.first_token_us') AS first_token_us
FROM model_calls c, json_each(c.usages) u
WHERE c.usages IS NOT NULL
UNION ALL
SELECT id, session_id, purpose, profile, model, state, started_at,
       input_tokens, output_tokens, total_tokens, NULL AS elapsed_us, NULL AS first_token_us
FROM model_calls WHERE usages IS NULL AND model IS NOT NULL;
