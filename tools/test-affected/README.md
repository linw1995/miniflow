# test-affected

Repository developer tooling for [issue #91](https://github.com/linw1995/miniflow/issues/91).
The unpublished `mfn-lsp` and `mfn-test-affected` packages share workspace checks but are not
product release dependencies, support packages, or bundled examples.

The planned Flow is `git.diff` (Task) -> `lsp.analyze` (Stream) -> `test_affected.plan` (Task).
It produces a test plan; a separate script consumes that plan. LSP servers are explicitly configured,
never downloaded automatically. Ordinary tests use a local fake server. Use `nix develop` for tools
and the pinned rust-analyzer when running real-server checks.

```sh
nix develop --command cargo check -p mfn-lsp -p mfn-test-affected
nix develop --command python3 -m unittest discover -s scripts -p test_tool_boundaries.py
```
