## ADDED Requirements

### Requirement: Transport caller-owned private tool arguments

The shared Agent and tool-dispatch Flows SHALL accept optional `params.tool_arguments: Map(Object)`, defaulting to an empty map. Its entries SHALL bind private startup inputs by business tool name without entering model-facing definitions or model-generated call records.

#### Scenario: Configure a tool from explicit application inputs

- **GIVEN** a caller supplies a private credential and endpoint for a configured tool
- **WHEN** the model requests that tool using only public arguments
- **THEN** dispatch merges the selected private values into ordinary Call arguments before tool execution
- **AND** ordinary Call binding validates types and gives explicit values precedence over environment/default sources
- **AND** the compiled tool body remains unchanged

#### Scenario: Reject unsupported private bindings

- **WHEN** private configuration names an unknown tool, an undeclared input, a public input, or a runtime-only input
- **THEN** dispatch rejects the configuration before executing any tool in the batch

#### Scenario: Prevent model credential overrides

- **GIVEN** a credential is supplied through private application configuration
- **WHEN** the model attempts to pass a secret input as a tool argument
- **THEN** dispatch returns the existing unknown-or-secret argument error without invoking the tool
- **AND** the model cannot replace the caller's private value

#### Scenario: Preserve per-tool secrecy

- **GIVEN** a shared private map contains credentials for one tool and another tool has no private binding
- **WHEN** both tools are requested
- **THEN** observations redact the private configuration and its selected call values
- **AND** unrelated public calls retain their existing visibility and value identity

#### Scenario: Preserve existing callers

- **WHEN** an existing caller omits private arguments
- **THEN** Agent and tool dispatch use an empty map and preserve existing public argument validation, tool execution, and result behavior

### Requirement: Separate web Agent provider configuration

The DeepSeek web Agent example SHALL expose distinct root `llm_model`, `llm_base_url`, `llm_api_key`, `deepseek_base_url`, and `deepseek_api_key` inputs. It SHALL NOT implicitly reuse a model credential for Search/Fetch or rely on hidden child environment settings.

#### Scenario: Execute a frozen web Agent with independent providers

- **GIVEN** the root receives distinct model and tool endpoints and credentials through explicit arguments
- **AND** environment fallback values differ from those explicit arguments
- **WHEN** a compiled Agent searches and fetches after its source files are removed
- **THEN** each provider receives only its own configured credential at its own endpoint
- **AND** model requests contain neither private tool values nor private input schema fields
