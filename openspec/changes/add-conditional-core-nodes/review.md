# Implementation Review

## Delivered behavior

- `mfn-core` registers constant, identity, and ordered if-else nodes through one explicit dependency.
- Data and control edges share deterministic DAG planning. Context references require an explicit ancestor path and never add scheduling edges.
- Effective instance metadata defines dynamic outputs and context references. Qualified IDs use `${node_id}.${output_name}` with exact lookup and collision rejection.
- Each run creates fresh execution state. Plugins receive a read-only `ExecutionContext` through `execute_with_context`; `ctx.output(id)` checks declared references and distinguishes values from skips and errors.
- Both in-memory execution and generated Rust use the same activation, publication, and selected-output helpers. Generated binaries retain direct calls with fixed bindings.
- If-else evaluates local structured conditions in order, stops at the first match, and emits one true activation value. Other outputs are explicitly skipped.
- Optional workflow results omit skipped sources while preserving null and rejecting unexpected missing outputs.

## Scenario evidence

| Requirements | Test evidence |
| --- | --- |
| Shared core registration and unchanged basic node behavior | `mfn-core` unit test `one_package_preserves_constant_and_identity_values`; migrated CLI compile tests |
| Dynamic instance ports, factory construction reuse, exact IDs and collisions | `mf-compiler/tests/context_planning.rs` |
| Direct/transitive dependencies, unordered/self/future references, mixed cycles | `mf-compiler/tests/context_planning.rs` |
| Context lifetime, pending/missing/skipped/null distinctions, atomic publication, low-level metadata errors | `mf-runtime/src/context.rs` tests; `context_execution.rs` |
| False/null activation, skip propagation, missing-output precedence, optional inputs | `mf-compiler/tests/context_execution.rs` |
| Third-party routing, no validation-time execution, memory/binary parity | `context_execution.rs` actual-binary fixture and `runner_validation.rs` |
| Optional omission, required skip errors, null preservation, missing and invalid selections | `context_execution.rs` and its generated-binary fixture |
| Branch IDs/counts, strict operators, pointers, literal validation | `mfn-core/src/if_else.rs` tests |
| Integer precision, signed/unsigned boundaries, decimal and exponent comparisons | `mfn-core/src/number.rs` tests |
| First match, fallback, short-circuit errors, nested branches, fanout, existence on skipped outputs | `mf-compiler/tests/if_else.rs` |
| Source/trigger separation, optional results, runnable examples | `examples/if-else.json`, `examples/else-if.json`, and `documented_examples_produce_the_documented_results` |
| Rebuilding after precedence changes, matching outputs and execution traces | `compiled_branches_match_memory_and_rebuild_when_precedence_changes` |
| Packaged support crates and installed CLI outside the checkout, standalone conditional binary | `mf-cli/tests/packaged_cli.rs` |

## Boundaries

Execution remains sequential. Multiple control dependencies are conjunctive, and ordinary nodes do not merge mutually exclusive branches. Predicate comparisons use runtime JSON numbers; the parser's existing precision limits still apply. Context values remain alive until the run finishes. Factory construction remains outside the business-execution skip guarantee.

The final integration pass tightened skipped-output lookup for low-level callers without resolved metadata and replaced the old test-only input resolver with a test of actual flow execution.

## Validation results

- `nix develop --command prek install`: hooks installed.
- `nix develop --command prek -a`: passed, including formatting, Clippy, Rust Analyzer, Markdown, and license checks.
- `nix develop --command bash scripts/run-cov.sh`: 97 tests passed, zero skipped, including installed CLI acceptance and generated-binary parity.
- `nix flake check -L`: passed on aarch64-darwin, including all 97 release-profile tests.
- `openspec validate add-conditional-core-nodes --strict`: passed.
- Coverage reports: `target/coverage/result/lcov.info` and `target/coverage/result/html/index.html`.

| File | Covered lines | Reported line coverage |
| --- | --- | --- |
| `mf-compiler/src/compiler.rs` | 429 / 429 | 100.00% |
| `mf-runtime/src/context.rs` | 386 / 390 | 98.97% |
| `mfn-core` condition implementation | 239 / 243 | 98.35% |
| `mfn-core` numeric comparison | 78 / 78 | 100.00% |

These per-file figures include colocated unit-test code. The current LLVM coverage profile emits no branch records; control-flow behavior is covered by the scenario tests listed above. Coverage review led to additional tests for repeated execution, invalid publication, metadata gaps, and low-level control dependencies.
