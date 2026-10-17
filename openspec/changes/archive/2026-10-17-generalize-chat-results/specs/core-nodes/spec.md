## ADDED Requirements

### Requirement: Concatenate explicitly supplied lists

The core package SHALL provide builtin.concat with required left and right JSON-list inputs and a value
list output. It SHALL preserve order, nested values, and sensitivity metadata without deduplication or
implicit collection from other nodes. Generated and in-memory execution SHALL use the same typed node.

#### Scenario: Combine model usage lists

- **WHEN** two nodes supply usage lists to concat
- **THEN** the output contains the left list followed by the right list without changing call identities

#### Scenario: Preserve shared sensitive values

- **WHEN** a list contains nested or secret values
- **THEN** concatenation preserves those values and their sensitivity rather than serializing them away
