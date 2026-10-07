## Context

See [proposal.md](proposal.md) for motivation and scope. There are no other active OpenSpec changes.

`mf-compiler/src/dependency_project.rs` generates a Cargo build script that loads linked providers and calls `CompiledWorkflow::generate_execution_plans`. That method already prepares the definition before emitting task and stream layouts. It currently discards the prepared interface after code generation.

`--describe` derives a structural description from the embedded plan without factories. `--describe-interface` prepares providers in a separate process. `mf-tui/src/run.rs::prepare_launch` consumes these documents to validate parameters, reject active stdin requirements, and select the observation protocol.

Graph descriptions deliberately omit configuration and complete port tables. `WorkflowInterface` already represents the needed startup contract and validates its relationship to a graph description. Supported release targets use ELF on Linux and Mach-O on macOS.

## Goals / Non-Goals

**Goals:**

- Make graph and interface inspection an executable-file read for new artifacts.
- Derive execution layouts and the interface from one validated build-time preparation.
- Preserve existing schema, observation, argument transport, and legacy-runner behavior.
- Detect generated startup-interface drift before node execution or source consumption.

**Non-Goals:**

- Removing inspection flags or legacy subprocess supervision in this change.
- Serializing executors, raw Rust objects, input values, configuration, or all internal node metadata.
- Changing dynamic in-memory Flow construction or adding a second runtime planner.
- Introducing sidecars, post-link executable rewriting, new cross-compilation support, PE support, or universal Mach-O packaging.
- Authenticating executable contents or changing the existing workflow identity algorithm.

## Decisions

### 1. Share one versioned manifest contract in the runtime

Add a runtime-owned contract containing a manifest version, `WorkflowDescription`, and `WorkflowInterface`. Re-export the shared types and required selectors through the existing runtime entry point. Keep existing graph/interface versions and validators; do not flatten or duplicate their fields into a new schema. The first payload version is `2026-10-07`.

Retain the graph and interface workflow IDs initially and require equality. The existing ID identifies the compiled definition, not plugin binary contents; it is not a substitute for checking the actual prepared schema.

Use a fixed byte header followed by UTF-8 JSON:

| Field | Encoding |
| --- | --- |
| Magic | Eight bytes: `MFMANIF\0` |
| Framing version | Four-byte unsigned little-endian integer, initially `1` |
| Payload length | Eight-byte unsigned little-endian integer |
| Payload | One supported JSON manifest document |

Decode fields explicitly without casting file bytes to Rust structs. Bound the combined JSON payload to 16 MiB; count both graph and interface against that budget. Reject unknown versions, duplicate JSON members, length overflow, incomplete payloads, and inconsistent graph/interface metadata. Permit only bounded zero alignment padding after the declared payload, with a section read budget of the header plus maximum payload plus 4 KiB. Configuration and supplied business values remain excluded.

The runtime owns framing and semantic validation; executable-container parsing stays in the TUI. Typed Snafu errors retain JSON and contract sources. This keeps the runtime independent of both object-format and terminal dependencies.

**Alternative:** separate graph and interface sections would require coordinating two records and duplicated discovery. One envelope preserves their relationship with one read.

### 2. Produce the manifest alongside execution layouts

Extend the generated-build artifact result to carry execution-plan Rust source and framed manifest bytes. Capture the derived startup schema from the existing validated preparation before prepared nodes are consumed or dropped. Preserve startup-input setup for both task and stream paths; older schemas receive the existing empty interface rather than new parameter promotion.

Do not generate the manifest by calling the current `describe_interface` helper after layout generation: that would repeat provider construction. Reuse the same prepared nodes and schema derivation rules for layout and manifest generation, including synchronous bodies and configuration-dependent ports. Graph description still comes from the same compiled definition and canonical order.

The generated `build.rs` writes the layout and manifest into `OUT_DIR`; the runner includes both during its existing compilation. Provider dependencies, aliases, features, lock resolution, and Cargo freshness remain shared with execution. Update the generated-project layout identity so old owned build directories cannot silently supply stale generated sources.

**Alternative:** a second compilation or post-validation rewrite complicates the existing one-build installation guarantee. Producing bytes in the existing build avoids both.

### 3. Embed a retained byte array in a dedicated section

Generate a fixed-length static byte array rather than a slice pointer or a Rust struct. Place it in `.mf_manifest` for ELF and `__DATA,__mf_manifest` for Mach-O using target-specific attributes in the generated runner. Target selection belongs to target compilation, not the host build script's platform.

Keep a real runner reference to the array for decoding and contract comparison. This reference retains the data through compilation and linking; an additional unused-static retention marker is unnecessary. Prove retention with release, LTO, and strip tests on the pinned Linux and macOS toolchains. Section lookup does not require a symbol table, and the retention test reads the actual section through the production reader.

Compatibility inspection commands decode this array and print the existing graph/interface JSON formats without preparing providers. Validation and execution read it through the same runtime decoder, ensuring the normal binary keeps a live reference to the data. No object-file parser is linked into the generated runner.

**Alternative:** a file trailer would require modifying final artifacts and coordinating signing/strip order. Link-time embedding fits the current Cargo output/install pipeline.

### 4. Read the executable directly and make absence distinct from corruption

Add an executable reader in `mf-tui` using the `object` crate with only the necessary read/ELF/Mach-O features. Use bounded file-backed reads for container headers, section tables, and the selected payload; do not apply the 16 MiB manifest limit to the entire executable or copy an unbounded executable into memory. Validate regular-file input, section ranges, checked offset arithmetic, and finite parser/read budgets. Require exactly one matching section, including its expected Mach-O segment.

Return either a validated manifest, a specific absent-section result for a successfully recognized supported executable, or a typed failure. Malformed containers, duplicate sections, unsupported container/payload versions, invalid ranges, and corrupt payloads are failures. They must not trigger command fallback or heuristic magic-byte scanning.

For a valid manifest, `prepare_launch` obtains the graph and interface together. Keep its existing argument validation, conditional stdin resolution, receiver-before-child ordering, input-file transport, and observation-version checks. Metadata preflight starts no process. An executable for another supported architecture can be inspected, while actual execution still requires a compatible host.

Only an absent section selects the existing bounded `--describe` and, when required, `--describe-interface` path. Preserve its deadlines, output limits, successful-exit requirement, and complete-document parsing. Legacy script-based test doubles must be replaced or exercised through the explicit legacy command helper; unrecognized files are not evidence of a missing section.

**Alternative:** falling back after any parsing failure hides corrupt new artifacts and makes file inspection unexpectedly execute a runner.

### 5. Compare freshly prepared startup contracts before dispatch

New generated runners decode the manifest and compare its schema to the startup schema produced by their ordinary runtime preparation. Compare exact initial-node and port identities, value types, required flags, and stdin ownership conditions. This check applies to both `--validate` and normal task/stream execution and must happen before executor dispatch, source reads, timer activation, or scheduling workers.

A mismatch is a typed preparation/contract failure with node/port/resource context. Keep authoritative argument and resource validation after the comparison. Do not reconstruct a graph, partition domains, or construct providers again to perform this check. Compatibility inspection intentionally returns the frozen contract without checking providers; `--validate` remains the operation that tests runtime preparation.

Provider interface declarations must depend on fixed configuration and the selected implementation, not environment variables or process state. Executor initialization may still depend on runtime resources and fail normally. Host and target builds may use distinct implementations of platform-specific code, but their exposed configured interfaces must agree. This change does not add cross-compilation; it catches interface differences in existing host/target preparation paths.

The new comparison covers the startup interface, not every internal node field. Existing validation and static-layout guarantees continue to own the rest of the workflow contract. In-memory callers keep dynamic preparation without an embedded-manifest requirement.

**Alternative:** trusting the embedded schema alone could let the TUI validate one interface while the executable uses another. Comparing the already-prepared schema closes that gap without another metadata pass.

## Risks / Trade-offs

- **Linker or packaging discards the section** -> Make the bytes reachable from validation/execution and exercise release, LTO, and actual strip tooling on Linux and macOS, including telemetry-disabled builds.
- **Provider schema depends on environment or target** -> Preserve configuration-derived declarations and reject build/runtime disagreement during validation and execution.
- **Container parsing uses unbounded memory** -> Bound file-backed header/table reads and section payload independently of executable size; test forged sizes and offsets.
- **Compatibility code remains** -> Keep legacy supervision isolated behind the absent-section result and verify corrupt new artifacts never spawn it. Flag removal is a separate migration.
- **Frozen schema contains configuration-derived names** -> Preserve the existing metadata boundary: expose type/port declarations, not configuration or argument values.
- **A path is replaced between inspection and launch** -> This change does not add descriptor-based execution or an executable authenticity guarantee; keep the existing local-path launch semantics.

## Migration Plan

1. Introduce the shared format and generation/retention support without changing legacy inspection dispatch.
2. Wire generated `--validate` and execution to schema comparison; switch compatibility commands to the embedded records.
3. Prefer file-based manifest loading in TUI preflight and retain bounded legacy fallback for absent sections.
4. Update compilation, observability, generated-project fixtures, and cache layout documentation. Require strict OpenSpec validation, pinned hooks, and Nix checks before implementation submission.

Existing binaries are not rewritten and remain usable through fallback. Source workflow versions and event schemas do not change. Keep the two inspection flags for existing clients; there is no removal deadline in this change. If file-based inspection must be rolled back, existing commands can still read new manifests, and older TUI releases can inspect new runners through those commands.
