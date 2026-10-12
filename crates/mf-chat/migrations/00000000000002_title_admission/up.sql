CREATE UNIQUE INDEX one_active_title_per_session ON model_calls(session_id)
    WHERE purpose = 'title' AND state IN ('pending', 'running');
