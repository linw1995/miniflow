## Why

Node value contracts lack common enum, default, nullable, numeric, and deferred payload codecs. Providers currently need
manual conversions that duplicate runtime validation and can lose presence or shared-payload semantics.

## What Changes

- Add string and internally tagged enum derives with strict payload conversion.
- Add opt-in field defaults, nullable values, and shared payload views that defer owned decoding.
- Support unsigned, target-sized unsigned, and single-precision numeric codecs with bounded descriptors.
- Preserve certified transfer equivalence for nullable values and defaulted fields.
- **BREAKING**: extend public descriptor and certified representation enums; exhaustive matches need new cases.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `typed-port-contracts`: extend value codecs while preserving strict conversion, presence, and transfer equivalence.

## Impact

Changes affect runtime codecs, derives, compiler descriptor handling and typed generation, and developer documentation.
The Code node retains its existing CEL type vocabulary. No dependencies or facade modules are added.
