## 1. Remove the semantic path

- [x] 1.1 Delete the `mfn-lsp` crate, the `lsp.analyze` node, and test-body isolation; connect `git.diff` directly to `test_affected.plan`; drop analysis fields from the plan with consumer compatibility.
- [x] 1.2 Remove `mfn-lsp` references from CI/Nightly/CD workflows, codecov flags, `run-cov.sh`, `coverage_selection.py`, and tool-boundary tests; rewrite flow and planning tests.
- [x] 1.3 Update openspec (this change), AGENTS.md, docs/ci.md, docs/node-development.md, docs/releases.md, and the tool README.
