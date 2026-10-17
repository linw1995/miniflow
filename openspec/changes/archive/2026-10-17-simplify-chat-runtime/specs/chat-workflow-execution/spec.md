## MODIFIED Requirements

### Requirement: Prepare the configured flow before beginning work

The host SHALL apply the selected nonsecret model configuration during flow preparation and validate node contracts before creating running work. Conversation and title configurations SHALL be independently selectable. Preparation MUST NOT read credentials, open the database, or contact providers. Each call SHALL retain its selected profile and actual model attribution.

#### Scenario: Select a different title model

- **WHEN** title generation uses a model configuration different from conversation generation
- **THEN** each workflow uses its selected model and records that identity with its own call

#### Scenario: Reject invalid preparation

- **WHEN** model configuration or a workflow node contract is invalid
- **THEN** preparation fails without creating a running turn or issuing a provider request
