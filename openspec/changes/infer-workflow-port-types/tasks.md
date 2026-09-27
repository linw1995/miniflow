# Tasks

## 1. Establish node evidence and literal inference

- [x] 1.1 Add an optional, declarative output-derivation API to `mf-runtime` with a no-op default; verify existing third-party node fixtures still compile and invalid port references or contradictory literal declarations fail metadata validation with node and port context.
- [x] 1.2 Add bounded JSON-to-`ValueType` inference for scalars and homogeneous nested collections; verify signed and unsigned integer boundaries, floating representation, empty and heterogeneous collections, nested maps, and the 16-level descriptor limit in runtime unit tests.
- [x] 1.3 Make `builtin.constant` provide its exact configured value and inferred output port type, and make `builtin.identity` declare an unchanged-input derivation; verify scalar, nested collection, null, and skipped-node behavior in core-node tests.
- [x] 1.4 Document the derivation contract, soundness requirement, and fallback behavior for ordinary plugins in `docs/node-development.md`; verify the example names the actual public API and retains a working no-derivation registration.

## 2. Resolve and validate graph types

- [x] 2.1 Resolve output type and optional exact-value facts in canonical topological order after structural and port-name checks; verify constant-to-identity chains, typed plugin sources, control-only edges, deterministic results, and no node execution during validation.
- [x] 2.2 Check exact source values against target port types before applying ordinary compatibility rules to unknown sources; verify direct and forwarded scalar conflicts, heterogeneous nested mismatch paths, null rejection, accepted empty collections, and unchanged runtime-checked `Any`/broad edges.
- [x] 2.3 Apply resolved output ports to in-memory `FlowNode` values and reject malformed derivations without weakening output guards; verify a falsely typed producer fails before publication and direct Flow execution retains its existing validation behavior.
- [x] 2.4 Update `docs/workflows.md` to distinguish known-value errors, inferred-type errors, and genuinely unknown runtime-checked edges; verify the documented constant-to-identity example has matching compile diagnostics and execution output.

## 3. Match generated runner behavior

- [ ] 3.1 Initialize generated nodes with the same shared evidence resolver and fixed generated bindings used by runner validation; verify the generated source retains direct execution order and contains no built-in kind-name inference.
- [ ] 3.2 Add generated-runner and packaged-CLI cases for homogeneous inference, identity propagation, inactive-branch conflicts, nested mismatch paths, and unknown plugin outputs; verify results and diagnostics match in-memory preparation and that validation calls no node execution method.
- [ ] 3.3 Verify a changed constant that conflicts with a typed input rejects the newly built runner, leaves the previous executable intact, and does not require a second build after successful validation; cover reused build directories in compiler integration tests.
- [ ] 3.4 Update `docs/compiling.md` and any affected examples to explain compile-time conflict diagnostics and executable preservation; verify their commands and expected outputs against the packaged CLI tests.

## 4. Integration checks

- [ ] 4.1 Run `nix develop --command bash scripts/run-cov.sh` and inspect coverage for literal inference, fact propagation, known conflicts, and generated-runner parity; add only missing behavior cases.
- [ ] 4.2 Run `nix develop --command prek install`, `nix develop --command prek -a`, `nix flake check -L`, and `openspec validate infer-workflow-port-types --strict`; verify all required checks pass before submission.
