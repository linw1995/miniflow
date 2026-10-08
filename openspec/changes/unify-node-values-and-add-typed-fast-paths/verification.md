# Verification

## Implemented behavior

- Unified owned port structs use `NodeValue`, with explicit adapters preserving the directional APIs.
- Providers opt into certified scalar/collection field generation through a shared typed task handle.
- Standard oneshot runners generate direct field transfers inside eligible serial domains; custom runners retain
  dynamic preparation and full context visibility.
- Target compilation checks constructor field tuples and exact port names. Preparation checks the complete resolved
  metadata before dispatch. Dynamic and typed strategies share one initialized provider instance.
- Payload snapshots and scoped execution retain dynamic invocation. All producer fields are validated before a
  successor, including unused floating descendants. Dependency presence, skips, errors, and scheduling use the common
  lifecycle.

## Evidence

- Existing codec and task regressions, unified derive compile cases, private-field constructor fixtures, and runtime
  documentation examples pass.
- Planner tests cover maximal chains, declaration-order stability, observed outputs, context reads, optional fields,
  refinements, distinct representations, control readers, fan-out, and dynamic providers.
- Lifecycle tests cover owned allocation identity, repeated invocations, invalid unused outputs, snapshot fallback,
  independent parallel typed domains, and missing-before-skip without input assembly.
- Generated external runners cover owned strings and lists, both telemetry modes, business-error equivalence, static
  constructor type/name rejection, target metadata drift, warm rebuilds, and preservation of installed executables.
- Documented workflow acceptance and the external OTEL integration regression pass. The minimal-plugin fixture now
  removes public module declarations as well as private ones.
- The nextest integration test uses the existing Cargo wrapper's source configuration once; it does not append a
  duplicate configuration file.
- Focused instrumented coverage ran 79 tests successfully. Input codecs reach 100% line coverage, output codecs
  94.38%, the typed planner 93.75%, and the derive implementation 80.10%. This selection intentionally does not claim
  whole-workspace coverage. Reports are local under `target/coverage/result/`, including `lcov.info`.
- Repository hooks, codegen-disabled compilation, and final complete Nix validation pass. The final pinned suite
  ran all 477 tests successfully on aarch64-darwin; other platforms were not executed locally.

## Local performance observations

The ignored `target/typed-fast-path-ablation/` directory contains the reproducible local harness, JSON measurements,
and report. A release microbenchmark uses 24 nodes, 64 KiB owned strings, 30 invocations, and one reused worker.

| Path | Allocated bytes | Allocation calls | Total execution, ms | Standard binary bytes |
| --- | ---: | ---: | ---: | ---: |
| Dynamic | 95,293,860 | 13,590 | 2.176 | 2,849,824 |
| Typed | 4,318,530 | 7,410 | 0.355 | 3,305,152 |
| Mixed | 39,909,360 | 9,810 | 1.002 | 3,211,680 |

The typed sample allocates about 95.5% fewer bytes and has a roughly 16% larger standard binary. Timing is a small
local observation, not a portable performance guarantee or test threshold. Build times are recorded in the local
report, but differing cache histories make them unsuitable for a comparative claim.

## Compatibility and scope

Exhaustive `NodeMetadata` literals require `typed_generation: None`. Standard custom project wiring should pair
`generate_runner_artifacts` with `generate_runner_execution_plans`; general-purpose callers keep their existing
artifact and plan methods. Existing directional derives and manual task contracts remain supported.

The first version certifies runtime-owned scalar and collection codecs. Connected optional fields, opaque custom
codecs, unproven refinements, value observers, fan-out, joins, cross-domain edges, streams, and nested bodies retain
dynamic conversion. Provider generation declarations remain contracts: business invariants belong in task execution,
and providers must expose the same business executor through ordinary and typed construction.
