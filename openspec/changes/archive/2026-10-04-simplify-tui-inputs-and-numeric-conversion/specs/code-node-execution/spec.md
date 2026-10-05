# Spec Delta

## ADDED Requirements

### Requirement: Explicitly convert numeric text

CEL Code SHALL accept `int(text)` and `double(text)` for declared string inputs and infer Int64 and
Float64 output ports respectively. Integers SHALL fit signed 64-bit range. Floating-point
conversion SHALL accept decimal and scientific notation and produce a finite JSON number. Invalid text,
integer overflow, or a nonfinite result MUST fail execution without publishing Code outputs. Conversion SHALL be explicit; input ports retain strict type checks.

#### Scenario: Convert a text integer

- **WHEN** string input `text` is `"42"` and the expression is `int(text)`
- **THEN** the node emits JSON integer 42 through an inferred Int64 port

#### Scenario: Convert a text double

- **WHEN** string input `text` is `"1e3"` and the expression is `double(text)`
- **THEN** the node emits finite JSON floating-point value 1000.0 through an inferred Float64 port

#### Scenario: Reject invalid numeric text

- **WHEN** conversion receives invalid text, an overflowing integer, or a nonfinite floating-point value
- **THEN** execution fails without publishing Code outputs
