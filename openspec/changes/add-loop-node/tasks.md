# Tasks

## 1. Define and validate structured Loop graphs

- [x] 1.1 Add the `2026-09-29` definition variant, typed Loop/body/variable/condition structures, reserved engine kinds, and shared type descriptor parsing with Code's existing concrete subset preserved. Preserve `2026-09-26` parsing and serialization behavior; reject Loop constructs in the old version.
- [x] 1.2 Extend structural planning recursively with per-scope IDs, the synthetic `$loop` source, nesting and count limits, local edge validation, and deterministic body order. Cover cycles, cross-scope references, invalid assignment/exit placement, and `until` validation without loading plugins.
- [x] 1.3 Extend runner validation and type inference to resolve all ordinary body plugins, typed state ports, assignment inputs, and context references. Validate inactive branches and reject an initial-value fact that would incorrectly specialize a later pass.

## 2. Execute Loop frames in both engines

- [x] 2.1 Add scoped outputs, typed variable maps, an execution budget, and engine-owned assignment/exit operations to `mf-runtime`. Keep plugin context access read-only and preserve existing flat-flow behavior.
- [x] 2.2 Execute a body with fresh outputs on every pass, persistent variables, post-pass `until`, maximum-count success, and immediate exit. Verify skipped Loop activation, skipped assignment/exit, nested scopes, wrong types, missing outputs, early failures, and no publication of partial Loop results.
- [x] 2.3 Generate structured Rust loops and prepare plugin instances once. Run the same fixtures in memory and as standalone binaries, comparing results, pass counts, skipped results, errors, and execution budgets; verify generated binaries run without source or Cargo.

## 3. Update observation and terminal presentation

- [ ] 3.1 Add versioned nested descriptions, per-invocation lifecycle identities, pass start/finish boundaries, bounded event counts, and stop reasons. Verify old protocol fixtures and old-binary description handling remain valid.
- [ ] 3.2 Instrument Loop and body steps in the shared executor. Verify live events, failure paths, early exit, skipped Loop, skip causes, nested pass paths, sequence gaps, trace correlation, and absence of business values in emitted records.
- [ ] 3.3 Extend the TUI reducer and graph view with active-pass status, bounded recent-pass history, aggregate counts, and missing-event uncertainty. Verify repeated invocations do not conflict or regress, older detail eviction is visible, and process outcomes remain independent of telemetry completeness.

## 4. Document and validate delivery

- [ ] 4.1 Add a runnable Loop example and document the schema, variable scope, ordering, stop behavior, limits, diagnostics, and relationship to Dify Loop and Iteration.
- [ ] 4.2 Cover parser, planner, runtime, generated runner, packaged CLI, observation, and TUI acceptance scenarios in this change's specs. Check that invalid definitions never replace an existing binary or lock.
- [ ] 4.3 In `nix develop`, run `prek install`, `prek -a`, and `nix flake check -L`. Run coverage with `nix develop --command bash scripts/run-cov.sh` where execution or reducer paths need coverage evidence.
- [ ] 4.4 Run `openspec validate add-loop-node --strict` and map every specification scenario to an implementation check before archiving this change.
