## ADDED Requirements

### Requirement: Derive model arguments from unconfigured Flow inputs

The shared Agent SHALL derive model tool arguments only from unconfigured public inputs in the tool capability's interface. It SHALL NOT require a separate application-private argument map or a tool-specific merge node.

#### Scenario: Configure a public setting before model publication

- **GIVEN** a tool Flow has a public setting configured from a producer-context template
- **WHEN** its model schema is projected
- **THEN** that setting is absent from properties and required arguments
- **AND** the model may supply only the remaining public inputs
- **AND** a model attempt to override the configured setting becomes an argument error

#### Scenario: Keep credential configuration outside the model

- **GIVEN** a tool capability has a key and endpoint configured from separate root provider settings
- **WHEN** the Agent invokes that tool
- **THEN** only its native invocation receives the configured values
- **AND** model schemas and model-generated call records do not contain those values
- **AND** secret markers protect configured credentials without hiding unrelated public calls

#### Scenario: Preserve fallback parameters

- **GIVEN** an unconfigured public input declares an environment source or default
- **WHEN** its model schema is projected
- **THEN** the input remains available as an optional model argument
- **AND** omitting it uses ordinary Call fallback resolution

#### Scenario: Keep declaration documentation without exposing fixed inputs

- **WHEN** tool documentation includes a configured public input
- **THEN** catalog validation accepts that declared documentation but omits the fixed field from the model schema
- **AND** unknown or secret documentation fields remain invalid
