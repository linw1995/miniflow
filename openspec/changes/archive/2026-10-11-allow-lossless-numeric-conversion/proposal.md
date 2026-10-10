## Why

Numeric codecs reject representation changes even when they preserve the value, so an integer cannot feed a floating
field and an integral float cannot feed an integer. Single-precision narrowing also rounds silently.

## What Changes

- Allow implicit numeric conversion only when value, range, precision, and negative-zero sign are preserved.
- Use the same conversion rules in descriptors, owned codecs, Code inputs, and graph validation.
- Keep numeric edges checked where exactness depends on the value; retain Rust representation checks for direct moves.
- Reject lossy single-precision narrowing, fractional integer conversions, and saturated integer boundary casts.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `typed-port-contracts`: permit exact numeric conversions and reject precision loss.
- `code-node-execution`: use the same exact conversions for existing CEL numeric inputs.

## Impact

Changes affect runtime numeric helpers, validators, codecs, Code input conversion, regression coverage, and documentation.
The CEL type vocabulary is unchanged. Callers that relied on lossy f32 narrowing must perform an explicit conversion.
No dependencies or facade modules are added.
