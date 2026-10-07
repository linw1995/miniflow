## ADDED Requirements

### Requirement: Preserve typed Loop assignment validation causes

Loop assignment validation failures SHALL retain the original `TypeMismatch` through the runtime error source chain and identify the assignment target variable. Callers SHALL be able to inspect the mismatch path, expected type, and actual type. A failed validation MUST NOT stage a variable write.

#### Scenario: Preserve a nested assignment mismatch

- **WHEN** an assignment to a `List(Map(Int64))` variable receives `[{"count": "wrong"}]`
- **THEN** execution identifies the target variable, retains the typed mismatch with path `/0/count`, expected type `Int64`, and actual type `string`, and stages no write
