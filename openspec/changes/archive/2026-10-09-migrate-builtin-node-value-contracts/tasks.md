## 1. Fixed builtin contracts

- [x] 1.1 Migrate Constant to `NodeValue` and `TypedTaskNode`, preserving literal evidence and output refinements.
- [x] 1.2 Migrate Batch and Readline fixed bags to `NodeValue`, preserving broad metadata, timing, ownership, and error sources.
- [x] 1.3 Review configured-port nodes and document their dynamic boundary without adding artificial schemas.

## 2. Builtin generation and completion

- [x] 2.1 Advertise certified Identity generation through an exported constructor and one shared typed handle.
- [x] 2.2 Extend existing behavioral regressions with builtin generation, refined fallback, invalid inputs, and payload identity.
- [x] 2.3 Review AGENTS.md compliance, ablate unnecessary code/tests, run pinned hooks, complete Nix checks, focused coverage, and strict change validation.
- [x] 2.4 Commit each implementation group, archive the reviewed change, validate main specifications, and update the current PR.

## Archive outcome

The fixed-contract group was committed as `c706702`, and the reviewed generation group was committed as
`6897575` before archival. The archive adds two requirements to `typed-port-contracts`. All nineteen main
specifications pass normal validation. PR #114 carries the follow-up implementation and archive.

The current archive passes task validation. Bulk archive validation retains the unrelated pre-existing
`2026-10-05-unify-flow-runtime` archive with seventeen incomplete tasks.
