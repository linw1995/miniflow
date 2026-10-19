## 1. Versioned definition and canonical graph

- [x] 1.1 Add version `2026-10-19` and strict version-selected decoding through JSON and programmatic paths; inherit existing features and reject mixed-format root/body fields.
- [x] 1.2 Add input bindings and execution-edge syntax; reject duplicate input keys, malformed variants, null/empty output names, data fields on new edges, and new-version `control_edges`.
- [x] 1.3 Normalize both formats into data bindings, output gates, and node-success prerequisites before analysis, preserving source serialization and supported body boundaries.
- [x] 1.4 Audit structural/prepared/generated/stream/body/description/instantiation paths; reject executable success edges until runtime support is installed in group 2.

## 2. Node-success semantics and execution

- [x] 2.1 Add typed prerequisites and frame-local authoritative Task outcomes; publish success after output validation and record skips independently of outputs and observation.
- [x] 2.2 Integrate resolution, error-before-skip precedence, bounded retention, concurrent-domain synchronization, and generated/typed execution without synthetic outputs or another scheduler.
- [x] 2.3 Add validated success-edge builder support and restrict success edges to same-scope/frame ordinary Tasks; reject stateful, container, synthetic, and cross-domain endpoints while preserving existing output-gate support.
- [x] 2.4 Update activation facts, context ancestry, startup-input eligibility, and mixed cycles; distinguish router success from branch-output production in exclusive proofs.
- [x] 2.5 Test zero-output success, skipped optional outputs, validation failure, skips, errors, cancellation, pending/missing outcomes, conjunction, and frame resets with observation disabled.
- [x] 2.6 Verify in-memory execution, generated dependency emission, typed data paths, concurrent domains, body Tasks, stream-message frames, and isolated execution; complete standalone parity with the protocol in group 3.

## 3. Protocol and diagnostics

- [ ] 3.1 Add versioned node-success edges to root/body descriptions; update validation, graph identity, manifests, readers, and protocol selection.
- [ ] 3.2 Add versioned node-success skip causes; update OTLP/event encoding, verbose output, and reducers without sentinel ports or payload exposure.
- [ ] 3.3 Retain old protocols for representable graphs; test standalone/in-memory success parity, old/new preflight, nested dependencies, round trips, and observation uncertainty.

## 4. TUI projection and inspection

- [ ] 4.1 Add Execution, Dependencies, and Ports views over full topology, distinguishing node-success edges from output gates and data mappings.
- [ ] 4.2 Route and size only visible connections; add deterministic data bundles, member details, mixed/unknown styles, hidden counts, and bounded lazy caching.
- [ ] 4.3 Add selected-node prerequisites and graph-owned `e` navigation/help in ordinary/chat hosts; preserve selection, viewports, and redaction.
- [ ] 4.4 Test 18-port pairs, mixed dependency kinds, data-only graphs, old manifests, uncertainty, body/view navigation, stable geometry, and routing budgets.

## 5. Migration and integration validation

- [ ] 5.1 Document version migration, success-versus-output semantics, Task scope, and unchanged config references; migrate one conditional example and add zero-output ordering coverage.
- [ ] 5.2 Add legacy/new equivalence and malformed-version fixtures without bulk source rewrites.
- [ ] 5.3 Run `openspec validate add-node-input-references --strict`, `nix develop --command prek install`, and `nix develop --command prek -a`; explicitly lint untracked documentation.
- [ ] 5.4 Run `nix develop --command python3 scripts/affected_tests.py --worktree --run`; preserve empty Rust selection and run affected non-Rust checks separately.
- [ ] 5.5 Review all deltas, compatibility, success publication, and exclusive-proof boundaries; archive only after implementation and review.
