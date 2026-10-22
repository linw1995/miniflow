CREATE TABLE records (
    commit_sha TEXT NOT NULL PRIMARY KEY,
    run_id TEXT NOT NULL,
    attempt BIGINT NOT NULL,
    schema_version BIGINT NOT NULL,
    signature TEXT NOT NULL,
    created_at BIGINT NOT NULL
);

CREATE TABLE tests (
    id INTEGER NOT NULL PRIMARY KEY,
    package TEXT NOT NULL,
    binary TEXT NOT NULL,
    name TEXT NOT NULL,
    UNIQUE (package, binary, name)
);

CREATE TABLE files (
    id INTEGER NOT NULL PRIMARY KEY,
    path TEXT NOT NULL UNIQUE
);

CREATE TABLE coverage (
    commit_sha TEXT NOT NULL REFERENCES records(commit_sha),
    file_id INTEGER NOT NULL REFERENCES files(id),
    line_start BIGINT NOT NULL,
    line_end BIGINT NOT NULL,
    test_id INTEGER NOT NULL REFERENCES tests(id)
);

CREATE INDEX coverage_range ON coverage(commit_sha, file_id, line_start, line_end);
