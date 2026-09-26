# Ablation Review

## Method

Baseline: `d4b8e8fa6ce516a4028a9b65ebcb7e246ce8b7b9`. Each accepted change was applied sequentially and checked with the same workflow behavior suite. Tests of deleted APIs were removed only with those APIs. Rejected candidates and fault injections were restored before continuing. Raw measurements are in [ablation-results.json](ablation-results.json).

```sh
nix develop --command cargo test -p mf-runtime -p mfn-core -p mf-compiler \
  --lib --test compiled_workflow --test context_planning \
  --test context_execution --test if_else
```

Line counts cover Rust files under `crates/`, splitting integration tests and inline test modules from production code. Public type counts cover `pub struct` and `pub enum` declarations. Timings include compilation and changing Cargo caches; they are observations, not performance benchmarks.

## Accepted changes

| Variant | Production lines | Test lines | Test functions | Public types | Behavior suite |
| --- | --- | --- | --- | --- | --- |
| Baseline | 3600 | 4057 | 97 | 48 | Passed |
| A: remove unused runner helpers and flow error wrapper | 3518 | 4036 | 96 | 47 | Passed |
| B: require metadata and unify port descriptors | 3479 | 4041 | 96 | 46 | Passed |
| C: use a flat completed-output context | 3388 | 3817 | 90 | 45 | Passed |
| D: consolidate generated-binary tests | 3388 | 3787 | 89 | 45 | Passed |
| Final: retain output-publication guard assertion | 3388 | 3788 | 89 | 45 | Passed |

After coverage review, one undeclared-output assertion was folded into the existing parameterized runtime test. The final result removes 212 production lines, 269 test lines, three public types, and eight tests. B initially exposed one missed `Cow<str>` to `String` conversion during migration; after correcting it, the behavior suite passed.

- Remove `execute_node`, `required_output`, and the separate `instantiate_node` wrapper; the generated runner uses the metadata-aware path.
- Return `WorkflowRunError` directly from both execution paths.
- Require `NodePorts` when constructing `FlowNode`. Use one `PortSpec` for borrowed static and owned dynamic names.
- Store qualified output values and explicit skip markers in one context map. Remove runtime registration, reverse indexing, pending-node records, and read whitelists.
- Retain compile-time metadata, collision, and ancestor checks. Native plugins still declare references for ordering validation; runtime context access is read-only but is not a permission boundary.

## Rejected removals

| Removal | Counterexample | Decision |
| --- | --- | --- |
| Replace exact numeric comparison with `f64` comparison | `9007199254740993` and `9007199254740992` became equal | Keep decimal normalization and boundary tests |
| Remove the strict ancestor check | A configured reference without an incoming dependency was accepted | Keep compiler reference validation |

Both experiments failed their existing regression tests with exit code 101. They were reverted before the next variant.

## Test ablation

The old runner-helper test exercised only unused entry points. Six context unit tests primarily exercised registration, private publication, whitelist, and metadata-free construction paths. Their workflow-level obligations remain covered by compiler validation, context execution, and conditional-node tests.

The two generated-binary harnesses both detected a deliberate code-generation mutation that dropped control edges. Consolidate them into one matrix and retain independent expected traces, third-party explicit skips, an inactive failure node, produced null, all branches, validation without execution, and precedence edits. The same mutation still fails the retained matrix after consolidation.

Retained checks cover missing versus skipped values, required/optional selected results, null preservation, explicit dependency ordering, qualified output collisions, strict conditions, exact numeric comparisons, branch fanout, and packaged standalone execution.

## API boundary

The generated step helper operates on a validated plan in topological order. It no longer supports a standalone registration protocol or detects arbitrary repeated manual calls. Applications should prepare flows through compiler APIs. Direct Rust constructors must provide complete metadata; configured workflow semantics and ordinary node implementations remain unchanged.

## Final validation

- `prek -a`: passed.
- `scripts/run-cov.sh` in `nix develop`: 89 tests passed, zero skipped; packaged CLI and standalone conditional execution passed.
- `nix flake check -L`: passed on aarch64-darwin, including the release-profile suite.
- `openspec validate add-conditional-core-nodes --strict`: passed.
- `context.rs`: 125/125 reported production lines covered. Numeric comparison: 78/78 reported lines covered, including colocated tests. No branch records are emitted by the current LLVM coverage configuration.

The behavior-suite timings decreased from 25.14 seconds at baseline to 7.87 seconds for the final run, but cache and test-matrix differences prevent attributing that change to runtime performance. The supported conclusions are reduced code/API surface and preserved workflow regression detection.
