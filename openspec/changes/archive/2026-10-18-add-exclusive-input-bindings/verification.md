## Validation scope

Implementation groups were committed independently on top of main commit `09ee0a5c2f2dbb4f93c8b6aaaaf9cd3bca73f54f`. Initial cumulative validation compared that base with implementation commit `ff06e50`. Subsequent verification results are recorded separately below.

## Results

- `openspec validate add-exclusive-input-bindings --strict`: passed before archive.
- `nix develop --command prek install`: installed both required hooks.
- `nix develop --command prek -a`: all checks passed, including formatting, Clippy, Cargo check, rust-analyzer, Markdown, workflows, and the license audit.
- `nix develop --command python3 scripts/affected_tests.py --worktree --run`: correctly selected no tests on the clean committed checkout.
- `nix develop --command python3 scripts/affected_tests.py --base 09ee0a5c2f2dbb4f93c8b6aaaaf9cd3bca73f54f --head HEAD --run`: conservatively selected all ten packages because the new example is a shared input. All 753 tests passed; four existing isolated-subprocess entry points were skipped by direct nextest scheduling.
- The final additional `exclusive_startup_input_activates_a_stream_producer` regression passed independently, covering both candidate branches into a Stream consumer.

The cumulative suite includes generated Task and Stream parity, loop and iteration bodies, embedding, isolated nodes, typed plans, metadata contracts, secret redaction, terminal graph rendering, and CLI examples. The example acceptance test compiles `exclusive-inputs.json` once and exercises both modes through `mf run`.

## Evidence and review

The focused tests additionally hold a candidate pending, then complete, fail, omit its output, or cancel it; no partial selection invokes the consumer. Stream batch tests switch winners across messages and preserve buffering deadlines when all candidates skip. Observation tests run with trace sampling disabled and verify selected identities and logical skip causes without exposing secret payloads.

See [review.md](review.md) for the contract review and integration findings. Raw cumulative test and hook logs remain under ignored `target/`. No benchmark or coverage percentage is claimed.

## Follow-up validation

The final implementation cleanup passed 25 focused tests, including generated stream and typed
runner acceptance. The affected-package selector then selected eight packages from the worktree
changes; all 714 tests passed, with four existing isolated-subprocess entry points skipped by
direct nextest scheduling. `prek -a` and strict validation of the archived feature's current
specification passed. No specification behavior changed, and the existing archive remains authoritative.

## Edge marker revision

The final definition uses `exclusive: true` on existing data edges, grouped by target node and
input. All 25 focused migration tests passed, including generated Task and Stream execution.
The affected selector conservatively chose all ten packages after the example changed: all
753 tests passed, with four existing isolated-subprocess entry points skipped by direct
scheduling. Repository hooks and strict feature-spec validation passed. The archived design,
proposal, and specification now describe the marked-edge format.

## Iteration provenance verification

The existing scoped-selection regression now compares unobserved and observed execution and
checks each sequential and parallel iteration item's selected source. The repository coverage
script, focused on that regression, reports all 28 executable lines of the iteration selection
log block covered. All 517 selected affected-package tests passed, with three existing isolated
subprocess entry points skipped by direct scheduling. Repository hooks passed.
