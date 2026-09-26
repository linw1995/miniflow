# Proposal

## Why

Flows need ordered `if` / `else-if` / `else` routing that prevents unselected downstream nodes from executing. The current executors run every node and treat absent outputs as errors; basic nodes also occupy separate packages despite sharing the same dependencies.

## What Changes

- Consolidate basic nodes into `mfn-core`, retaining `builtin.constant` and `builtin.identity` and adding `builtin.if_else`.
- Separate incoming execution dependencies from condition data access. Add explicit control edges alongside existing data-binding edges and a read-only execution context containing prior node outcomes.
- Require at least one configured condition branch. A node with `N` conditions has no required data input and exactly `N + 1` outputs, including a mandatory fallback `else` output.
- Publish context outputs under qualified IDs `${node_id}.${output_name}`, such as `load_order.value`. Runtime publication adds the node prefix; nodes keep local output names.
- Evaluate simple comparisons against qualified context output IDs and JSON Pointer paths. Stop at the first matching branch, activate its output, and explicitly skip the remaining outputs.
- Support configuration-dependent instance ports while preserving static registrations for existing plugins.
- Add explicit branch and node skip states, shared by in-memory execution and generated Rust orchestration. Preserve errors for unintentionally missing outputs.
- Add opt-in optional workflow outputs so unselected branch results can be omitted without confusing skips with JSON `null`.
- **BREAKING**: Replace the workspace's `mfn-constant` and `mfn-identity` packages with `mfn-core`; migrate package declarations and adjacent locks. Existing kind names and ordinary node implementations remain valid.
- **BREAKING**: Reject workflows whose distinct node/output pairs produce the same qualified output ID, rather than allowing ambiguous context references or overwriting stored values.

## Capabilities

### New Capabilities

- `core-nodes`: Provide basic nodes through one explicitly selected package, with stable kind names and dependency boundaries.
- `conditional-flow-execution`: Separate execution dependencies from context references, evaluate ordered conditions, activate one branch, propagate explicit skips, and select branch-dependent workflow outputs.

### Modified Capabilities

- `workflow-binary-compilation`: Plan data and control dependencies, validate effective instance ports and context references, and generate conditional orchestration with the same behavior as in-memory execution.

## Impact

- `mf-runtime`: additive context-aware execution and instance-port APIs, per-run context storage, control edges, shared skip handling, and optional output definitions.
- `mf-compiler`: combined dependency planning, instance-aware port/reference validation, construction reuse, and generated execution guards with context publication.
- Built-in crates, workspace dependencies, fixtures, lockfiles, examples, release package checks, and workflow/plugin documentation.
- Existing plugins can retain their static ports and `execute` implementations when rebuilt against the matching runtime. New conditional features require matching updated support packages.
- No script expression language, compound boolean expression tree, field-to-field comparison, branch merge node, loops, or concurrent scheduler is included.
