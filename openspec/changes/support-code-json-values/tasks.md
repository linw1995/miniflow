# Tasks

## 1. Extend Code JSON contracts

- [x] 1.1 Extend input and inferred-result mappings for dynamic JSON and uint; verify reflected ports for Any, Object,
  Array, nested collections, and `List(Object)` concatenation, retaining existing concrete-type checks.
- [x] 1.2 Accept explicit `dyn(...)` and remove obsolete dynamic-type rejection tests; verify unknown names,
  unsupported inferred types, and invalid concrete operations still fail construction, including inactive branches.
- [x] 1.3 Document supported descriptors, inferred broad outputs, and deferred dynamic checks in docs/workflows.md;
  verify examples match actual reflected ports and accepted expressions.

## 2. Preserve values, sensitivity, and limits

- [ ] 2.1 Extend recursive conversions with dynamic dispatch and UInt support; verify nested heterogeneous values,
  empty containers, null versus absent fields, unsigned maximum, large integers, and negative floating zero.
- [ ] 2.2 Reuse budgets and contextual failures for dynamic descendants; verify non-JSON results, non-string keys,
  nested errors, depth/entry/byte limits, and atomic failure of a multi-output invocation.
- [ ] 2.3 Explicitly inherit input sensitivity in the Code provider; verify direct execution and workflow observations
  redact outputs derived from secret descendants, independent outputs follow the conservative policy, and public
  inputs remain public. Document the policy without changing the shared runtime contract.

## 3. Verify workflow integration and migrate concat

- [ ] 3.1 Verify in-memory and generated runner execution with a structured-list fixture, including an empty seed,
  `List(Map(Any))` to `List(Object)` compatibility, and a failing Any-to-concrete consumer binding.
- [ ] 3.2 Integrate the chat prerequisite through the normal repository workflow before removal; confirm concat,
  chat-loop-flow.json, and the exact core requirement exist, then reconcile this delta with the integrated base.
- [ ] 3.3 Replace all concat consumers with Code and update package declarations, workflow locks, and migration docs;
  verify multi-node and repeated-loop usage collection preserves IDs, paths, omitted counts, errors, and order.
- [ ] 3.4 Remove concat registration, implementation, exports, and exclusive tests after migration passes; verify no
  active workflow or code references remain, keeping archived specs as history. Include a bounded loop accumulation
  regression proving over-limit execution fails without publishing a partial list.

## 4. Review and repository validation

- [ ] 4.1 Review against AGENTS.md and perform focused ablation controls for numeric preservation, secret propagation,
  dynamic traversal limits, and structured-list acceptance; verify retained tests detect each removed guard, remove
  redundant abstractions/tests, and keep experiment artifacts only under ignored target/.
- [ ] 4.2 Run `nix develop --command prek install`, `nix develop --command prek -a`, and
  `nix develop --command python3 scripts/affected_tests.py --worktree --run`; verify all required checks pass and
  honor the selector's conservative fallback and empty-selection behavior. Run affected non-Rust checks separately.
- [ ] 4.3 Validate the final implementation against every changed requirement and run
  `openspec validate support-code-json-values --strict --no-interactive`; confirm migration prerequisites are resolved
  and the removal delta can be applied to the integrated main specs before marking implementation complete.

## Workflow follow-up

- Archive only after implementation, migration, validation, and review are complete; retain this change unarchived
  while it is a plan. Do not mark implementation tasks complete for planning artifacts alone.
- Commit or publish only when requested; this plan does not authorize a push or pull request.
