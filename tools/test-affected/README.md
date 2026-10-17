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

## LSP node

`lsp.analyze` is a `TypedStreamNode`. Its input ports are `request` and opaque `context`;
its outputs are `outcome` and the unchanged `context` (including secret metadata).
A request identifies snapshots by `id`, absolute `root`, and `documents` with relative paths
and UTF-8 byte ranges. `initialization_options` is forwarded to the server unchanged.

The node configuration selects `command`, optional `args`, optional source `extensions`,
and `profile` (`standard` by default, or `rust_analyzer` for its explicit readiness/related-test
extensions). Limits default to 120000 ms, 2048 requests per snapshot, and 16 MiB per result budget.
The deadline spans the entire batch. Each snapshot gets one session, reused for all its queries.

Every completed batch emits exactly one `analyzed`, `incomplete`, or `not_requested` outcome.
Unsupported capabilities and remote analysis failures are evidence gaps, not proof that tests are
independent. Empty batches do not start a server. Cancellation, invalid requests, cleanup failures,
and emitter failures stop the node without publishing fallback. All child processes are reaped
before the terminal result is admitted. Progress adds diagnostic span events, never business messages.
The initial transport uses Unix nonblocking pipes; other platforms report an unsupported transport.

```sh
nix develop --command cargo nextest run -p mfn-lsp
```
