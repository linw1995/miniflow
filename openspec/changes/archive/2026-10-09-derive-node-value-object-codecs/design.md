## Context

Collections already implement their value codecs when their elements implement the corresponding codec. The unified
derive currently supplies only port-bag conversion. A derived struct needs an object codec to satisfy that existing
collection contract.

## Decisions

- Generate explicit codec and required-input-field implementations on the derived type. Use the existing runtime path
  and field bounds. Keep directional derives and manual field implementations independent.
- Describe the struct as `Object`, and reuse `NodeValue::from_values` and `into_values` for field validation. Object
  conversion clones shared value handles rather than rebuilding a JSON tree. Nested optional fields keep omission rules.
- Keep object codecs outside the runtime-owned certified representations. Object-shaped JSON does not prove a custom
  struct is eligible for direct typed transfer.
- Extend `TypeMismatch` with source-bearing input and output variants. Keep mismatch details as the target of
  `Deref`/`DerefMut` so existing field reads and collection path prefixing continue to work. Store scalar details
  directly; box nested details and sources to bound recursive error layouts. Use Snafu selectors for all contexts.
- Report structural failures against the object's broad descriptor and retain the original port error as the source.
  Invalid field values retain their precise expected and actual types. Avoid re-enumerating every declared port while
  handling an error.
- Export the conversion helpers through the existing runtime entry point because downstream derive expansions need
  them. Keep their error selectors within the existing runtime boundary. Add no facade modules or blanket codecs.

## Compatibility

Derived types acquire codec implementations, so providers that manually implemented those same codecs must remove the
duplicates. `TypeMismatch` field reads remain supported; construction and struct patterns must use its enum variants.
Manual `NodeValue` implementations do not acquire codecs automatically.

## Validation

Reuse existing round-trip, empty-struct, alias, generic, and diagnostic regressions. Cover nested objects in lists and
maps, optional object fields, shared identity, structural input failures, non-finite output descendants, and typed depth
sources. Run reversible simplification experiments and negative controls, keeping their records under ignored `target/`.
Run pinned hooks, complete Nix checks, focused coverage, and strict OpenSpec validation before committing and archiving.
