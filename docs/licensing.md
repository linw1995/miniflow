# Dependency license audit

Generate the dependency license report with the pinned development environment:

```sh
nix develop --command bash scripts/generate-third-party-notices.sh
```

The report is written to `target/THIRD_PARTY_NOTICES.html`. `cargo-about` checks the license allowlist in [about.toml](../about.toml) and fails on unaccepted or unresolved licenses. The audit covers all workspace crates with all features, including build and transitive dependencies, across the four release targets. Development dependencies are excluded.

`nix develop --command prek -a` runs the audit locally. In CI, the Nix package check generates the report offline using vendored dependencies and fails on unaccepted or unresolved licenses. Release archives include `LICENSE` and `THIRD_PARTY_NOTICES.html`; Nix packages install both under `share/licenses/miniflow/`.
The workspace audit includes the optional `mfn-code` crate and its CEL evaluator dependency, `cel-core`.
It also includes the optional `mfn-llm` crate, Rig provider contracts and HTTP transport, and their transitive dependencies.
The `mf-chat` core adds Diesel, embedded migrations, database ownership locking, and bundled SQLite to the workspace audit.

Generated workflow executables link the node packages selected by their Flow definition and the support packages used for validation and execution. Audit the generated Cargo project retained in the build directory and include the applicable dependency notices when distributing its executable. The repository audit alone does not cover third-party Nodes selected by another Flow.
