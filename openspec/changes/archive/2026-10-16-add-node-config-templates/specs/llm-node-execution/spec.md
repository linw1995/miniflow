## ADDED Requirements

### Requirement: Configure LLM invocations from runtime templates

LLM model, endpoint, generation parameters, deadline, and reply limit SHALL accept
shared configuration templates. An api_key template SHALL supply an execution-time
credential and MUST NOT coexist with api_key_env. Literal credentials SHALL be
rejected. Existing literal api_key_env configuration SHALL remain supported, with
OPENAI_API_KEY as its default when no credential template is configured.

#### Scenario: Configure the provider from typed parameters

- **WHEN** an LLM node references parameter outputs with an explicit predecessor dependency
- **THEN** execution uses their resolved values for the provider request and secret parameter snapshots remain redacted

#### Scenario: Reuse a prepared node with new settings

- **WHEN** a subsequent invocation supplies a different credential, endpoint, model, or limit
- **THEN** the reused node uses the new settings without retaining invocation-specific settings in its shared client

#### Scenario: Reject invalid runtime settings

- **WHEN** a referenced configuration value has the wrong type or violates a field constraint
- **THEN** execution fails before contacting the provider and retains safe typed error sources

## MODIFIED Requirements

### Requirement: Validate static generation configuration

The node SHALL require a model field and validate literal settings during preparation, rejecting unknown fields, literal credentials, unsupported options, and invalid types or ranges. Supported fields MAY use templates: preparation SHALL validate syntax and declare references; invocation SHALL validate resolved values before network I/O. Contracts MUST remain independent of the environment and resolved values.

#### Scenario: Use default connection settings

- **WHEN** a configuration supplies only a valid model
- **THEN** it selects the OpenAI API prefix, `OPENAI_API_KEY`, a 60000 millisecond deadline, and a 1048576 byte output text limit

#### Scenario: Reject invalid configuration

- **WHEN** literal configuration contains an empty model, unsupported URL, URL credentials, a nonfinite or out-of-range temperature, a nonpositive limit, an unrepresentable numeric value, or an unknown field
- **THEN** preparation fails with configuration context without acquiring external resources
