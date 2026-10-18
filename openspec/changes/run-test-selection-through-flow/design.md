## One definition and one executable

Keep `tools/test-affected/flows/test-affected.json` as the sole planning definition. Local and CI callers use the same compiled executable through `mf run`. They supply different parameters; the graph chooses the branch. Do not introduce separate local/CI definitions or a Python dispatcher.

```text
builtin.parameters -> builtin.if_else
                         |-- local: builtin.code ---------------------------|
                         |-- CI: evidence access + Code + Loop + if-else ----|--> git.diff.request
                                                                                 exclusive edges
                                                                                      |
                                                                                  lsp.analyze
                                                                                      |
                                                                               test_affected.plan
```

The CI label describes nodes in this same definition and its ordinary Loop body. It is not a new node kind or separately invoked workflow. Use `builtin.identity` only where the input already has the required shape; use Code for real transformations.

## Reuse inventory and upstream dependency

Integrate fetched origin/main `01e5e1c` before implementation, without rewriting published PR history. Its exclusive-input support proves that both request producers are under distinct outputs of one if-else router. Its structured JSON Code supports object, array, Any, and mixed JSON transformations.

- `builtin.parameters`: arguments, environment bindings, defaults, and secret metadata.
- `builtin.if_else`: local/CI routing, candidate decisions, and final disposition.
- `builtin.code`: small named predicates and transformations, including request construction.
- `workflow.loop` and existing loop state assignment: bounded candidate search and early stopping.
- Exclusive input edges: join alternatives at one downstream input, without a merge node.
- `git.diff`, `lsp.analyze`, `test_affected.plan`: the common comparison, analysis, and selection path.

Do not assume a CEL sort, assertion, error-catching primitive, or pre-tested Loop condition exists. Current Loop conditions are checked after a body pass, so empty candidate lists must be gated before indexing or entering the search. Runtime maximum-iteration termination does not itself mean successful selection. These behaviors must be demonstrated in the first composition gate.

## Inputs and common output

Public parameters identify repository, local comparison mode, base/head or commit, and requested precision. Local defaults to worktree comparison. A public CI flag may bind `GITHUB_ACTIONS`, with an explicit invocation argument taking precedence. CI additionally supplies run/attempt, PR, tested-merge identity, execution settings, and optional cumulative override.

CI-only startup parameters must be nullable/defaulted so local execution needs no GitHub setup. Only the active CI path validates them. Credentials must flow only to the relevant I/O invocation, not through public Code nodes or candidate/decision state: Code marks every output secret when any supplied input is secret.

Both branches produce one normalized comparison request containing correlated mode/base/head and provenance. Bind this complete object to the Git request input using exclusive edges; do not select correlated fields independently. Keep convergence before the LSP producer. Git and LSP must forward provenance through the producer boundary so the planner never reads an earlier message domain directly.

Output one test plan with explanations and, in CI mode, comparison/current-execution evidence for subsequent validation. Invalid execution must return failure, not a successful empty stream. An intentional empty test selection remains an explicit valid plan.

## Local branch

Code assembles the local request. Git resolves worktree, first-parent commit, merge-base, or direct-snapshot mode. Worktree mode captures staged, unstaged, and untracked nonignored files in an immutable runnable snapshot without changing the caller's index, files, or branch. Define an empty baseline for root commits.

Keep snapshots owned and available for the existing plan consumer. Preserve explicit-ref invocations and reject conflicting input forms. Local planning performs no GitHub access and never fetches missing history implicitly.

## CI branch: policy is graph composition

### Current identity and candidate metadata

Obtain actual local commit/tree/parents and GitHub PR/run/attempt facts. Verify the tested two-parent merge, actual target base, PR head, repository, workflow identity, and execution settings before accepting historical reuse or cumulative fallback. Missing or inconsistent current identity is fatal.

Acquire a bounded list of candidate run metadata. Use Git ancestry facts to exclude unrelated heads and establish the same nearest-ancestor ordering as the existing selector, including deterministic run/attempt tie breaks. Do not rank by completion time or assume API order is source order. Candidate metadata may be batched; artifact bodies and absent Git objects must remain lazy.

### Bounded search and early stopping

Maintain explicit Loop state for the remaining candidate/index, accepted comparison if any, reason, search completion, and stop disposition. If there are no candidates, select cumulative comparison without entering a body that indexes an empty list. Start no candidate search when cumulative mode is explicitly requested.

For each candidate, the Flow performs these operations:

1. Read only that candidate's attempt-bound artifact.
2. Evaluate cheap identity, status, schema/version, signature, and coverage-consistency predicates in small Code nodes.
3. If the record can still qualify, obtain missing candidate Git facts through an explicit bounded retrieval path.
4. Verify candidate parents/tree/ancestry against the platform and record claims.
5. Accept the request and set the stop condition, or advance to the next candidate with an explanatory disposition.

After acceptance, no later artifact read or historical fetch may execute. Do not replace this behavior with eager artifact download or parallel candidate execution. Ordinary per-candidate unavailability may permit another usable ancestor. Search truncation or exhausted bounds must be distinguished from successful exhaustion and produce the documented cumulative fallback with a reason.

Use existing Loop/if-else/state mechanisms for traversal. Keep comparison assembly and compatibility predicates visible in the Flow rather than inside a Rust baseline provider or one large CEL expression. Preserve current behavior with differential fixtures before retiring the Python implementation.

### Decision outcomes

- Compatible usable record: compare its tested merge snapshot directly with the current tested merge.
- No usable historical record, incompatible target base/settings, or bounded history failure: cumulative affected comparison against the current actual target base.
- Invalid current identity, cancellation, malformed invocation, or an internal Flow error: fail without publishing a plan.

A cumulative reset does not itself mean full-workspace testing. The existing planner still applies ownership, reverse dependencies, deferred inputs, and conservative fallback.

## Minimal external capability contracts

Freeze exact kinds and ports only after the fixture composition identifies the missing I/O. The permitted capability areas are:

| Capability | Facts or effects | Excluded policy |
| --- | --- | --- |
| GitHub execution access | Read PR/run/attempt metadata and bounded candidate pages, with source provenance | Baseline eligibility, candidate ranking, cumulative fallback |
| GitHub artifact access | Read a requested artifact/member, decode bounded JSON, and report absence/invalid transport data | Selection of a reusable validation record |
| Git revision access | Resolve refs, read trees/parents/ancestry/distance, report missing objects, and explicitly retrieve a requested object when authorized | GitHub workflow success or test-selection decisions |

These are operations to cover, not a requirement for one kind or crate per row or endpoint. Reuse existing Git helpers and extend existing provider contracts where cohesive. Add only minimal I/O kinds when an existing kind cannot express the operation clearly. Keep them in repository developer tooling; do not build a transport registry or wrap the old Python algorithm in a node.

I/O boundaries own response/archive decoding, origin/run binding, size bounds, process cleanup, cancellation, and Git integrity. Expected historical absence must be representable as data that the Flow can branch on. Cancellation and invalid current execution remain failures. The first gate must prove these distinctions through actual node/runtime contracts; no simulated success on errors or accidental CEL evaluation faults as an assertion mechanism.

Historical Git retrieval must be explicit, scoped to configured sources and requested IDs, and preserve branch refs, index, and worktree. Keep `git.diff` local by default. Every remote operation has a timeout, and the whole search has a bounded item and elapsed budget; specify how elapsed-budget evidence reaches Flow decisions without hiding the search loop in a provider.

## Design gate before production providers

Build a minimal executable composition against upstream using recorded/constant evidence and normal parameter inputs. Demonstrate same-definition local/CI routing, candidate selection, early exit, explicit fallback, missing/failed record handling, and current-identity failure. Exercise compiled and in-memory behavior, including exclusive-input proof across the chosen Loop wiring.

Document concrete I/O ports, availability/error values, limits, and the resulting minimal kind inventory from that composition. Also verify candidate ordering can be implemented using existing CEL and Loop facilities within current budgets. If explicit failure or required timing semantics cannot be represented, document the precise gap and revise this spec with the smallest reusable contract before adding production code. Do not proceed by recreating a baseline-policy node.

## Shared planning and CI migration

Use the existing Rust planner as the sole owner of file classification, known shared consumers, reverse dependencies, isolated-test refinement, and fallback. Compare its Cargo metadata graph with the current Python analyzer graph plus metadata supplementation; explain any changed selection conservatively rather than assuming equivalence.

CI initially requests package-only plans. Local callers may request semantic refinement. Package-only requests can pass an empty semantic batch through the existing `not_requested` Stream outcome without losing original Git changes. Real analyzer failures retain current fallback behavior; no pre-/post-producer bypass edge is allowed.

Use an explicit selection-policy version in compatibility checks. If migration changes execution or selection semantics, the first new run must reset to cumulative comparison rather than reuse an incompatible old record.

Replace `pr_validation.py select` with `mf` invocation and thin result extraction. Keep parameter transport, shell redirection, GitHub output export, and artifact placement as adapters. Update AGENTS.md and documentation, then remove duplicate Python selection/discovery rules and their superseded tests. If an old CLI wrapper remains temporarily, it forwards only and contains no independent selection logic.

Keep `run-tests.py`, `run-cov.sh`, coverage evaluation, and final attempt-bound publication outside this planning Flow. The final job still requires all checks and matching coverage/source evidence. Do not pass exact-case plans to a package-only coverage consumer or weaken the 95% gate. Main/nightly retain full verification.

## Validation and ablation

Use the same recorded histories and Git fixtures to compare selection, baseline provenance, fallbacks, record reads, and Git fetch counts against the old algorithm. Separately measure compilation, initialization, evidence acquisition, semantic analysis, planning, and test execution. The first valid candidate must suppress all subsequent artifact reads and fetches.

Reuse existing integration fixtures for local worktree preservation, root commits, missing refs, secret handling, current identity, failed/cancelled attempts, target-base reset, retries, expiry, no usable record, force-push ancestry, explicit cumulative mode, and empty plans. Cover changed Flow integration rather than copying upstream Code/exclusive-edge tests.

Ablate compatibility checks, ancestor ordering, early-stop wiring, explicit error propagation, and local branch isolation. Remove redundant graph steps, wrappers, and tests. Keep evidence under ignored `target/`. Run the pinned toolchain and required hooks, review AGENTS.md compliance, and archive only after implementation and integration validation complete.
