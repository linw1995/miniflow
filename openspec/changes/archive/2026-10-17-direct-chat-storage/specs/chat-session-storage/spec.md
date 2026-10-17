## MODIFIED Requirements

### Requirement: Expose application storage operations as nodes

The `mf-chat` package SHALL expose typed `mf_chat.*` nodes for initialization, session management, turn transitions, title persistence, and usage queries. Direct database APIs and node adapters SHALL call the same storage handlers. Host storage operations MUST NOT construct or execute wrapper workflows. Preparing or inspecting these nodes MUST NOT open storage, apply migrations, or execute queries.

#### Scenario: Prepare without a database

- **WHEN** a structurally valid chat workflow is prepared or inspected with a nonexistent database path
- **THEN** its node contracts are available without creating database files or applying migrations

#### Scenario: Return a direct storage error

- **WHEN** a database API fails to find a turn or execute a storage query
- **THEN** it returns the typed storage error directly without runtime error wrapping

#### Scenario: Attribute a storage failure

- **WHEN** a storage node fails during execution
- **THEN** the error identifies the operation and preserves the database error source
