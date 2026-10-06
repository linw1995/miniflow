## ADDED Requirements

### Requirement: Preserve typed runtime port validation sources

Runtime input and output port validation failures returned as `WorkflowRunError` SHALL retain the original `TypeMismatch` as their error source, together with the node ID and input or output port name. Callers SHALL be able to inspect the failing JSON Pointer, expected type, and actual type through that source. Diagnostic formatting SHALL NOT replace the underlying error with a string.

#### Scenario: Preserve an invalid producer output

- **WHEN** a plugin declares an `Int64` output but produces a JSON string
- **THEN** execution fails with the producer node and output name, retains a `TypeMismatch` source, and publishes none of that node's outputs

#### Scenario: Preserve a nested dynamic input mismatch

- **WHEN** a `List(Map(Int64))` input receives `[{"count": 1}, {"count": "two"}]`
- **THEN** execution fails before invoking the consumer, identifies its node and input name, and retains a `TypeMismatch` source with path `/1/count`, expected type `Int64`, and actual type `string`
