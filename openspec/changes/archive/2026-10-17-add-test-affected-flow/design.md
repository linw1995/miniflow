# Design

## Context

See [proposal.md](proposal.md) for motivation. The current package selector is the baseline reference. Local experiments demonstrated useful test-only refinement and missing LSP relationships across subprocesses. They also exposed the cost of semantic analysis that later proves unnecessary; this simple pipeline does not promise an early test-policy gate. Raw experiment results remain under ignored `target/`.

The runtime already supplies Task and Stream executors. A Stream producer runs on a dedicated worker, accepts serial invocations, and emits through bounded admission. Event callbacks run synchronously on the coordinator; they are unsuitable for waiting on LSP I/O. Stream emissions create a new message domain, so a downstream task cannot join them with a direct edge from the original input domain.

The checked-in `ci-test-selection` specification predates the active selector described by `AGENTS.md` and `docs/ci.md`. This change leaves that selector and CI policy in place. Reconcile the existing specification separately before a future CI migration.

## Goals / Non-Goals

**Goals:**

- Make LSP analysis an explicit, reusable node in a small developer Flow.
- Produce a conservative, inspectable test plan without executing tests inside the Flow.
- Reuse existing streaming lifecycle, cancellation, backpressure, and provider contracts.
- Keep workspace development checks separate from product releases.

**Non-Goals:**

- Publishing the tool as a primary product capability or replacing CI selection.
- A test execution node or a node for every policy function, LSP request, or reporting step.
- A generic project/test adapter registry or a second production language implementation.
- Persistent semantic caches, cross-node session pooling, or versioned intermediate protocol frameworks.
- A new release configuration system, server installation, automatic Git fetches, or uncommitted-worktree analysis.

## Decisions

### 1. Keep the tool under its use case

```text
tools/test-affected/
  README.md
  run-tests.py
  nodes/
    lsp/
      Cargo.toml
      src/
      tests/
    test-affected/
      Cargo.toml
      src/
      tests/
  flows/
    test-affected.json
```

Use package names `mfn-lsp` and `mfn-test-affected`. Add `tools/test-affected/nodes/*` to the development workspace, retain the CLI default member, and disable publication for both packages. Their versions need not follow CLI releases. Keep existing built-in paths unchanged; product packages must not acquire normal, build, or development dependencies on the tools.

Update current release entry points to explicitly exclude the tool packages from release tests, support checks, artifacts, and notices. Preserve notices for shared product dependencies and retain workspace-wide development license checks. Reuse existing package-selection helpers where possible rather than adding `.config/release-packages.toml` or a general release-policy registry. The shared lockfile can still include tool dependencies.

### 2. Use three nodes with one data path

```mermaid
flowchart LR
    G["git.diff / Task"] --> L["lsp.analyze / Stream"]
    L --> P["test_affected.plan / Task"]
```

The Flow uses schema `2026-10-03` and `execution.mode = stream`, even for a single finite analysis job. The initial Task receives startup inputs once. The LSP producer receives that job, emits its terminal result in a new message domain, and the final Task consumes only that result. Normal streaming completion waits for downstream delivery and execution to drain.

There is no direct `git.diff -> test_affected.plan` edge, no execution/reporting node, and no Event executor. A negative compilation fixture verifies that adding the cross-domain bypass is rejected by the existing runtime.

### 3. Make the envelope the boundary

The LSP node binds source snapshots and ranges from the Git outputs and combines them with its own query/profile configuration. `AnalysisRequest` below is its logical batch model; the Git node does not construct LSP protocol messages or select semantic operations. The logical input/output shape is:

```text
Input:
  request: AnalysisRequest
  context: ValueRef

Output:
  outcome: AnalysisOutcome
  context: ValueRef
```

`AnalysisRequest` describes snapshot roots and identities, changed document ranges, and initialization
options. Node configuration selects the server, readiness profile, source extensions, and analysis budget; the
provider issues a fixed set of standard semantic queries. `AnalysisOutcome` contains a status,
snapshot-attributed evidence, gap/failure reasons, and analysis timings. Status is `analyzed`, `incomplete`,
or `not_requested`; `analyzed` means the requested operations finished, not that every dependency is known.

`context` is passed through unchanged, including runtime value metadata. It is not parsed, logged, or used to make policy decisions by `mfn-lsp`. The tool stores its job identity, original Git changes, immutable snapshots, and planning configuration there. The Git node does not compute a test baseline or classify test code. The final Task validates that outcome snapshot identities agree with this context.

Use ordinary Rust value types and existing node value contracts. Do not add live process handles to ports or require an independently versioned schema for each internal record. Final plans and reports include the source/configuration identity needed to detect stale input.

### 4. Let Git supply before/after facts

`git.diff` resolves local commits, computes their merge base and old/new paths and ranges, and materializes clean snapshots without changing the caller's checkout. It emits source changes and snapshot descriptions suitable for the configured analysis batch, plus the original comparison information in opaque context. Generic source-range conversion does not introduce Cargo or test-selection decisions into the Git node.

`mfn-test-affected` initially owns this node alongside the plan node. Do not introduce a separate Git provider package until another actual use case warrants extraction. The node does not launch LSP, inspect package dependencies, or decide which tests are necessary.

The initial Flow is intentionally linear. Full-workspace policy fallback is discovered in the final plan node, so a manifest/shared-input change accompanied by source edits can still incur LSP cost. Empty or inapplicable semantic batches can avoid server startup, but a general test-policy preflight gate is deferred. Measure this trade-off rather than moving policy into Git or the generic LSP provider.

### 5. Analyze as an input-driven Stream producer

Register `lsp.analyze` through `PreparedNode::typed_stream` and `TypedStreamNode`. The producer receives one finite batch per invocation; requests within one snapshot/configuration share a session. Base and head use separate sessions. The first version closes owned sessions at the end of each batch instead of pooling them across invocations.

The producer handles stdio framing, response correlation, server requests, notifications, partial results,
advertised capabilities, and negotiated position encoding. Generic protocol/query code knows nothing about
Cargo or test selection. The Rust profile adds server-specific readiness; semantic queries use statically
advertised standard capabilities, and unsupported operations become attributed gaps. Do not add dynamic
capability registration or extension-result registries before a consumer needs them. The unused `relatedTests`
query was removed after paired historical replays preserved selection and standard relationship counts.
Standard-only server fixtures verify the generic path without a second project adapter framework.

A successful, incomplete, or unrequested batch produces exactly one terminal emission. LSP progress notifications use observations/diagnostics, never extra business emissions. An empty batch emits `not_requested` with the original context without starting a server. A missing server, deadline, or server crash emits `incomplete` with structured reasons after cleanup, allowing the final Task to apply fallback.

Invalid request contracts, inability to complete required cleanup, and runtime failures fail the producer. Cancellation or instance disposal must interrupt pipe reads and waits, terminate/reap children, and return without a fallback result. Checking cancellation only at the emitter is insufficient while waiting for an idle server. Sends use the existing emitter; admission errors are propagated and never retried as a second business result.

Complete analysis and session cleanup before the single terminal emission, because downstream execution can start before the producer returns. No fallible analysis remains after emission. Upstream closure finishes admitted work through the existing producer lifecycle; it does not cancel an in-flight batch. Test this explicitly for the startup Task that emits once and then completes.

### 6. Produce a plan without executing tests

`test_affected.plan` validates the forwarded comparison and analysis identities. It owns concrete Cargo metadata, Rust test-classification, and plan-construction modules. Selection, union, fallback, and formatting remain ordinary functions inside this final Task.

Derive the package baseline from Cargo metadata with every declared local dependency kind retained, and verify parity with successful results from the current selector. Missing workspace metadata is an error. Unusable dependency analysis with known workspace membership, manifests, shared configuration, and unknown inputs retain full-workspace fallback. Recognized documentation-only changes retain an empty plan.

Use Git change ranges as the starting point and base/head semantics as supporting evidence. Comparing only semantic graph differences is insufficient: changing a function body can affect tests without changing any call edges. Handle deleted and renamed definitions using both revisions; final executable identities refer to head.

Narrow isolated test-body changes only with verified ownership and unchanged surrounding production/setup code. Shared helpers, reused test functions, registrations, ambiguous macros, and missing subprocess edges retain wider scope. Production changes keep their reverse-dependent packages. Empty reference sets do not authorize exclusions; analyzer startup/deadline/workspace failures cause full fallback, and unsupported optional queries retain unproved scopes.

The output is a TestPlan containing source/configuration identity, retained package or binary scopes, exact test selectors where justified, baseline fallback, and reasons. It can use a supplied matching head inventory to enumerate concrete cases. Without one, it marks unenumerated scopes explicitly; the Flow never builds test binaries or executes test bodies to produce a count.

The #151-shaped case retains its two changed runtime tests and every test in `mfn-llm` and its consumers. The #157 subprocess counterexample prevents treating missing LSP edges as exclusions. Rules are not keyed by PR number.

### 7. Execute the plan through a small script

`tools/test-affected/run-tests.py` consumes the completed plan artifact and invokes nextest outside the Flow. Reuse existing command and coverage helpers where their contracts fit, without a new runner framework. The current coverage helper consumes package-only plans; it must not silently widen or ignore exact-case selection from a refined plan.

The script validates snapshot and configuration identities, lists tests when required, checks exact target/case identities, and reuses the build for execution. Ambiguous or unmatched cases widen to the recorded baseline with a diagnostic; stale identities or malformed plans fail. An empty plan runs no test command. Real compile/test failures preserve diagnostics and a nonzero exit status rather than becoming selection recovery.

The Flow has no execution mode and does not launch this script. Reporting is an ordinary plan output or script result, with no report node. Plans describe selection as structured data rather than a shell command to evaluate.

Snapshots live under an explicitly owned artifact directory in ignored `target/` and outlive the Git node, the Flow, and any later script consumption. They are not RAII guards dropped when a node returns. Materialization validates content identity; cleanup is an explicit developer operation limited to owned paths. The script rejects plans whose referenced snapshots have been removed or changed.

### 8. Retain only the abstractions needed now

`mfn-lsp` is a general protocol/analysis provider. `mfn-test-affected` contains concrete Git, Cargo, Rust classification, inventory-mapping, and policy modules; the script owns process execution. Extract an adapter trait only when a second real implementation needs it. Standard-server protocol fixtures are necessary; fake non-Cargo project plugins are not.

Session reuse within an invocation is required. Persistent semantic caching is deferred until measured recomputation costs justify its invalidation complexity. Existing Cargo build reuse remains available. Errors use Snafu with typed sources in the owning tool crate; do not move tool errors or session state into the runtime.

### 9. Evaluate without replacing CI

Include both packages in development checks and applicable CI/nightly coverage lists. Keep the current selector, coverage flags, and full verification policy in place. Use the existing selector as a differential reference and independently validate final nextest identities.

Report Flow analysis/planning time separately from script inventory/build and test execution time. Benchmark the new three-node Flow rather than presenting old prototype timings as its performance. A smaller test count alone does not establish a wall-time improvement. Future CI adoption requires a separate decision and reconciliation of the existing CI specification.

## Risks / Trade-offs

- Cross-domain data loss -> carry opaque context through the Stream emission and prohibit bypass edges.
- Premature downstream planning -> emit one terminal result only after evidence and cleanup are final.
- Late full-fallback detection -> record its LSP cost honestly; defer a policy preflight gate until measurements justify it.
- Cancellation during pipe I/O -> bounded waits, existing cancellation signals, and child cleanup tests.
- False test-only classification -> verify ownership and surrounding code; retain uncertain scopes.
- Incomplete semantic relationships -> retain unproved tests and test subprocess/generated-code counterexamples.
- Release leakage -> test existing release entry points and product dependency closure without a new release framework.
- Snapshot lifecycle leaks -> keep owned, identifiable artifacts under `target/`; avoid per-node lifetime assumptions.

## Migration Plan

1. Add the two unpublished tool packages and narrow release exclusions, preserving development validation.
2. Implement the generic LSP Stream provider and lifecycle fixtures.
3. Implement Git and plan Tasks using concrete modules and ordinary policy functions, then a small script that consumes the plan.
4. Add the three-node streaming Flow and verify in-memory/generated execution, including empty requests and immediate upstream closure.
5. Replay locally available pinned #131, #151, #152, #157, and #159 snapshots, compare exact test identities, and verify script consumption separately. Keep portable synthetic fixtures independent of network history.
6. Measure analysis and execution costs separately before considering CI adoption. Keep raw artifacts under `target/` and implementation tasks unchecked until verified.
