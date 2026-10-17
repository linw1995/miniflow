# Review

No blocking implementation or specification findings remain.

- Admission depends on external conversation and result contracts. Internal model kinds, counts, and graph
  edges no longer determine execution mode or accounting. Source-compatible scalar usage remains only for
  existing single-call consumers and legacy stored facts.
- Metadata is object-valued, is included in history size accounting, and is saved on the new assistant
  message. Returned history cannot rewrite earlier annotations. Provider requests omit application metadata.
- Workflows explicitly combine usages. Repeated calls have unique IDs and node/scope attribution; actual
  per-call start timestamps preserve UTC filtering across long-running workflows.
- Request and conversion failures can become workflow data by explicit configuration. Valid reported counts
  survive failure, while absent counters and unreported invocations remain distinct from zero.
- Success and failure persistence use short transactions and exact execution ownership. Identical failed
  result replay is idempotent; committed success wins over late conflicting results.
- Existing database rows and indexes survive forward migration. The statistics view expands reported lists
  once and uses legacy scalar facts only when a list is absent.
- Runtime changes expose invocation identity without collecting application usage. Existing scope-prefix
  state is shared with snapshots and remains available in unobserved nested bodies. Identity is restored on
  errors and unwinding, and forked domains do not inherit an actively executing node.
- The typed SQL JSON adapter serves two concrete shapes and preserves typed error sources. Error-as-data
  handling is explicit; ordinary execution errors retain their original typed chains.
- Public entry points, workspace dependency policy, and node-owned errors follow AGENTS.md. The new generic
  concat node has no chat storage, provider, or expression-engine dependency.

Detailed ablation scripts and measurements remain local under Git-ignored `target/chat-flow-ablation/`.
