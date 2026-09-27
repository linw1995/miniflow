# Spec Delta

## ADDED Requirements

### Requirement: Infer core node output evidence

`builtin.constant` SHALL expose the exact configured JSON value and a sound port type for output `value`
during validation. It SHALL infer `Null`, `Boolean`, `String`, `Int64`, or `Float64` for representable
scalars, and `List(T)` or `Map(T)` for nonempty collections whose recursively inferred members all have the
same type. It SHALL use `Number` for integers outside signed 64-bit range and `Array` or `Object` for empty,
heterogeneous, or over-depth collections. `builtin.identity` SHALL pass the known type and exact-value
evidence of its bound input to output `value`. These facts SHALL describe an output only when it is produced
and MUST NOT change either node's JSON or skip behavior.

#### Scenario: Infer a scalar constant

- **WHEN** a constant is configured with signed JSON integer `42`
- **THEN** its `value` output is known to be `Int64` with exact value `42`

#### Scenario: Infer nested homogeneous collections

- **WHEN** a constant is configured with `[{"count": 1}, {"count": 2}]`
- **THEN** its `value` output has type `List(Map(Int64))` and retains that exact JSON value

#### Scenario: Broaden an unrepresentable collection without losing its value

- **WHEN** a constant is configured with `[1, "x"]` or an empty array
- **THEN** its `value` output has type `Array` and retains the exact configured value for connection validation

#### Scenario: Propagate through identity

- **WHEN** a constant feeds two identity nodes in sequence
- **THEN** the last identity output retains the constant's type and exact value whenever it is produced

#### Scenario: Preserve null and skips

- **WHEN** a constant configured with `null` feeds an identity node, or a control dependency skips that identity node
- **THEN** a produced value remains present null and a skipped output remains skipped
