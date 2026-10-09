## 1. Object codecs

- [x] 1.1 Generate object input/output codecs and required input fields with the existing runtime path and bounds.
- [x] 1.2 Reuse shared named-port conversion and retain directional Snafu sources and escaped nested paths.
- [x] 1.3 Extend existing regressions for collections, optional objects, shared values, structural errors, and depth limits.
- [x] 1.4 Document descriptors, codec compatibility, and the typed-generation boundary.

## 2. Review and completion

- [x] 2.1 Review AGENTS.md compliance and complete reversible production/test ablations with retained-guard controls.
- [x] 2.2 Run pinned hooks, full Nix checks, focused coverage, and strict change validation; resolve all findings.
- [x] 2.3 Record final review and validation evidence, then commit the implementation.

## Workflow follow-up

Archive the reviewed change after the implementation commit, synchronize its capability requirements, validate the
resulting main specifications, and commit the archive.

## Archive outcome

The reviewed implementation was committed as `cc1e2d7` before archival. The archive synchronized two added requirements
into `typed-port-contracts`. All nineteen main specifications pass validation, and this archive has no incomplete tasks.
