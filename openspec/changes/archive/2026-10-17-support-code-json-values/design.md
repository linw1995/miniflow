# Design

## Context

See proposal.md for motivation and the chat integration prerequisite. Code currently compiles expressions at node
construction and converts values using a concrete type whitelist. Its input and result mappings reject dynamic JSON.
The runtime already supports broad descriptors and checked connections, including `Map(Any)` to `Object`.
The pinned CEL dependency already provides dynamic values and generic list addition.

## Goals / Non-Goals

**Goals:** Preserve JSON business values when concatenating and transforming structured lists; expose sound port
contracts; keep concrete expressions statically checked; preserve secret protection and bounded conversion.

**Non-Goals:** Record schemas, chat-aware Code logic, automatic usage collection, expression dependency tracking,
fine-grained taint analysis, zero-copy CEL adapters, new execution backends, hard CPU isolation, or configurable limits.
This change does not add `number`, `nullable`, `usize`, or `float` input descriptors to Code.

## Decisions

### 1. Extend the existing type bridge

Use the existing recursive List and Map mappings with these additions:

| Runtime descriptor | CEL input type | Inferred CEL result to runtime |
| --- | --- | --- |
| `any` | `dyn` | `dyn` becomes `Any` |
| `object` | `map(string, dyn)` | `map(string, dyn)` becomes `Map(Any)` |
| `array` | `list(dyn)` | `list(dyn)` becomes `List(Any)` |
| `uint` | `uint` | `uint` becomes `Uint64` |

`List(Object)` inputs therefore concatenate into `List(Map(Any))`, which the existing compatibility rules allow
to feed `List(Object)` consumers. Preserve these sound structural types rather than special-casing the expression
or declaring all results `Any`. No explicit output schema is necessary.

Accept `dyn(...)` now that its results have a valid JSON boundary. Unknown identifiers, invalid calls, and provably
incompatible concrete operations still fail construction. Dynamic operations can fail during execution. Do not claim
that arbitrary `Any` contents or object fields are checked statically. Non-string-key inferred map types remain
unsupported; maps hidden behind dynamic results require runtime key validation.

A record-type registry would provide stronger field checking but is unnecessary for list concatenation and would
couple the runtime to a new schema system. Keeping the explicit `dyn` ban would add a special restriction without
protecting a boundary that declared dynamic inputs do not already cross.

### 2. Reuse recursive conversion with dynamic dispatch

Dispatch dynamic inputs by their actual JSON kind. Preserve null, booleans, strings, array order, object fields,
and empty containers. Missing fields stay absent. Map JSON signed-range integers to CEL Int, larger unsigned integers
to CEL UInt, and stored floating numbers to CEL Double. Do not route integers through floating point or parse strings
implicitly. Existing declared numeric inputs retain their shared lossless conversion rules.

Convert dynamic results by their actual CEL variant, including UInt. Reject non-finite numbers, non-string map keys,
bytes, timestamps, durations, opaque values, and nested evaluation errors rather than stringifying them. Preserve
typed CEL error sources where available and input/output plus escaped JSON Pointer context for conversion failures.
JSON equivalence is required; internal reference identity and object key serialization order are not.

### 3. Make sensitivity propagation an explicit Code policy

The Code provider will explicitly mark every produced output secret when any supplied input contains a secret,
using the existing derived-value operation. Apply this to both concrete and dynamic execution, including direct
task execution. This conservative policy protects values reconstructed through CEL, including derived scalars.

The shared runtime still does not infer sensitivity for arbitrary nodes. Code opts into this policy because it
does not track per-expression data dependencies. It can redact an output independent of the secret input; document
that trade-off. Fine-grained propagation and preserving public siblings are deferred, not prerequisites for migration.
This does not add a guarantee that arbitrary evaluation error strings are sanitized.

### 4. Preserve limits and publication behavior

Keep 8 KiB per expression, 1 MiB each for serialized inputs and outputs, 10,000 total collection entries across
input/output conversion, and 16 nesting levels. Dynamic descendants must traverse the same budgets; broad descriptors
must not bypass depth checks. A failing output prevents publication of every output from that invocation.

Concatenation now traverses nested values and reconstructs containers, unlike the dedicated node. Its accepted input
size is consequently smaller. Document this migration boundary and test repeated accumulation near the limits.
Do not remove guards, add a chat-specific exception, or infer a maximum turn count from entry limits.

### 5. Migrate only after the general capability works

Use the same `left`, `right`, and `value` ports to minimize graph edits:

```json
{
  "language": "cel",
  "inputs": {
    "left": {"list": "object"},
    "right": {"list": "object"}
  },
  "code": {"value": "left + right"}
}
```

For arbitrary heterogeneous lists, declare both inputs as `{"list":"any"}`. Keep call IDs, node paths, omitted
token counts, and errors unchanged. No deduplication, aggregation, or implicit discovery is introduced.
Keep chat result decoding responsible for validating `CallUsage` fields.

## Risks / Trade-offs

- Dynamic types defer some errors to execution. Mitigation: preserve concrete checks and validate complete results.
- Reconstructed values can lose secret markers. Mitigation: explicit provider propagation and redaction coverage.
- Conservative propagation can redact extra fields. Mitigation: document the policy without adding taint machinery.
- Accumulated usage can exceed Code limits. Mitigation: bounded success/failure tests and explicit migration notes.
- Removing concat breaks definitions using that kind. Mitigation: migrate all repository users and document package
  and configuration changes; do not add a compatibility alias that retains the duplicate node indefinitely.
- The chat prerequisite is absent here. Mitigation: gate removal and archive on integration of that implementation
  and its core requirement; re-read the integrated spec before applying the removal delta.

## Migration Plan

1. Implement and validate Code support independently using structured JSON fixtures.
2. Integrate the chat prerequisite through the normal repository workflow; reconcile its spec with this delta.
3. Replace concat definitions, declare `mfn-code`, regenerate affected workflow locks, and update documentation.
4. Verify generated and in-memory execution, multi-node collection, and loop accumulation before removing concat.
5. Review the complete change, run required checks, and only then archive. Planning does not complete these steps.

If migration fails, retain concat until the gap is resolved. Before release, rollback can restore the migrated
definitions and core registration together; no database migration is involved.
