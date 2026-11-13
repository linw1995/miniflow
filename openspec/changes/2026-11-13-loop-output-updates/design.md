# Loop body output updates

## Goal

Replace end-of-pass assignment plumbing with ordinary body output selections. Keep
`workflow.loop_assign` for writes that must become visible within a pass; do not
generalize assignment to other containers or change scope isolation.

## Contract

- A Loop body using definition version `2026-11-13` may select terminal outputs
  whose names are declared mutable Loop variables. Older bodies still require
  empty terminal outputs. Read-only inputs and intrinsic `index` are not targets.
- After a complete successful pass, validate all selected values against their
  variable types, apply the updates together, then evaluate `until`. Selections
  are references, not additional execution steps or scheduling dependencies.
- Unselected variables retain their values. An explicitly skipped selection with
  `optional: true` also retains its variable; required skipped outputs and missing
  producer outputs remain failures under ordinary Flow output rules.
- Body outputs cannot target a variable also written by `workflow.loop_assign`
  in that body. Immediate assignments to other variables remain supported.
- An explicit exit abandons end-of-pass outputs, including selections whose
  producers were not visited, while preserving completed immediate assignments.
- Failure, cancellation, invalid output types, or publication errors do not
  publish partial Loop outputs. Already executed external side effects are not
  rolled back. Type mismatch sources and pass failure observation are preserved.
- TaskStream exports remain separate under `stream_outputs`; they are never
  interpreted as terminal state updates.

## Implementation boundary

Reuse existing body output references and prepared terminal ports; no new facade,
synthetic assignment tasks, or second output transport is introduced. Lowering
checks the version gate. The owning Loop provider validates target names, type
compatibility, and conflicting immediate writes from actual prepared interfaces
and compiled relationships. It also validates actual updates before committing.
Finite Flow output selection is bypassed after an explicit scope exit so an
abandoned pass cannot fail on unvisited selections. Dynamic execution and generated
runners use that same selection boundary.

The Agent selects `messages`, `usages`, `text`, `outcome`, and `error` directly from
its producers and removes its five assignment nodes. Optional `messages` and
`error` selections retain the current skip semantics. Public ports and streaming,
ordering, failure, usage, and iteration-limit behavior remain unchanged.

## Validation

Cover multi-pass updates and termination, optional skips without stale values,
required skips and missing outputs, target/type/conflict rejection, runtime typed
mismatches and failed-pass observation, early exit, nesting, secret/native values,
saved plans, generated runners, and streaming parity. Run the shared affected-test
Flow and `prek -a` before submission.
