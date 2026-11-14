# Verification

## Implementation scope

ToolUse, canonical conversation recovery, event-driven collection, relational persistence and terminal presentation
are implemented together. This revision follows the review direction to use the Agent Flow's message contract
exclusively, without preserving historical `mf-chat` wire formats. Desktop materials remain ignored local artifacts.
License policy and notice generation are unchanged; no dependency version is updated.

## Single Agent contract

- `ConversationInputs.messages` and `ConversationResult.messages` are required typed lists, not optional alternatives.
  The Agent accepts only complete messages for conversation input. Its old prompt/system/history parameters and
  initial-message assembly node are removed; no public accumulator or persistence boundary counter is introduced.
- Terminal prompts remain host admission inputs. The host appends one user message to the last successful complete
  context, including initial configured system instructions only when starting a session. Flows receive no second
  conversation representation. Optional application context/title and usage extensions remain independent.
- Manifest checks reject missing/ambiguous message inputs and missing/optional message outputs before invocation.
  Both success and failure results require messages. Tests reject missing/null messages and text-only programmatic
  completions; explicit empty failed state is distinct from an omitted field.
- Settlement never constructs an assistant message from final text or result-level metadata. The returned final
  assistant message must match canonical text. Result replay and context loading use complete context versions,
  never streamed evidence or audit-row concatenation. Old saved rows remain inspectable, but missing canonical
  contexts cause explicit restoration/replay errors rather than fabricated compatibility results.
- Single-call, calculator, Loop, flat-title and nested-title examples and standalone callers use the same required
  message interface. The Loop appends actual assistant messages to one context, including intermediate successful
  rounds, without inserting another user prompt between model calls. Opaque message metadata remains message-owned.

## Recovery and evidence

- A real custom compiled Flow changes system instructions and compacts history. Database reopen and the next actual
  HTTP provider request prove that only its successful returned context plus new user input is used. Tests cover
  edited/reordered/reset history, repeated equal messages, old-version inspection and failed-version exclusion.
- Relational context membership preserves exact order and full content/metadata/tool arguments/results. Immutable
  session-scoped payload interning saves storage without merging message identities or inferring execution provenance.
  Sensitivity survives reopen independently of bounded/redacted display previews, including 130-call contexts.
- Immutable message events retain publisher-generated IDs independently of the final context. Compaction cannot
  erase their requests/results or explicit execution links. The reducer retains original event identity and full
  messages before checkpoint writes; failed-write Save-only recovery and later replay are proven zero-write.
- SQL triggers retain atomic settlement/rollback, zero-write replay, immutable payload sharing and status-only
  updates without sibling/payload rewrites. Migration-only fixtures seed historical records without reintroducing
  a production text-only completion path. Ownership and fail-fast contention tests remain enabled.
- Source/reducer/receiver tests cover both arrival orders, explicit/late/ambiguous correlation, reused provider IDs,
  multiple attempts, bootstrap/live handoff, closure draining and no-render delivery. Context-only tool projections
  remain labeled and never acquire invented execution provenance. Drawing performs no collection or database writes.
- Real calculator/provider/PTY tests preserve live tools before delayed final text with the Flow panel hidden,
  restart without execution, complete next-request context, reused provider IDs, cancellation, save-only retry,
  title accounting, empty stdout and terminal restoration. Graph geometry assertions remain enabled; example
  annotation text was preserved rather than weakening the layout regression.

## Final validation

Using the pinned Nix Rust 1.99.0 toolchain:

- Shared planning Flow rebuilt; immutable worktree selection at test precision with telemetry and `run=true`:
  **1275 passed, 5 skipped**, 143 binaries, six affected packages, no fallback.
  Source snapshot: `94682303206dd9959ae63b1a4edfebde2aa8074c`.
  `target/desktop-probe/agent-contract-test-plan.json` retains the executable plan and execution report.
- Focused documented-example and graph checks: **67 passed**; standalone stream/title and compiler callers were also
  checked after migrating their input flags. All are included in the full successful selection above.
- Repository Python checks: **76 passed**.
- Official coverage script with the affected plan and explicit test-name filters: **57 passed, 1214 filtered**.
  `target/coverage/result/lcov.info` is a partial focused report, not a full-workspace or complete-crate coverage claim.
- `prek -a` includes formatting, all-target/all-feature Clippy/Cargo check, Markdown, workflow validation and the
  unchanged license audit. Strict OpenSpec validation includes explicit modifications of the former split-input,
  result-reconciliation and history/metadata requirements.

Fixtures and expected messages were migrated to explicit full results rather than preserving a compatibility shim.
No test was deleted or assertion disabled to hide a failure.

## CI status

Previous head `bf434217` passed lint and Ubuntu checks but failed incremental coverage: 2999/3195 lines (93.87%),
below the required 95%. This revision does not weaken that gate. The updated head requires fresh CI and remains Draft.

Raw logs, plans, coverage reports and application databases stay under ignored `target/`; no desktop code, graphical
licensing material or machine-specific report is included in the PR.
