# Dependency license audit

Generate the dependency license report with the pinned development environment:

```sh
nix develop --command bash scripts/generate-third-party-notices.sh
```

The report is written to `target/THIRD_PARTY_NOTICES.html`. `cargo-about` checks the license allowlist in [about.toml](../about.toml) and fails on unaccepted or unresolved licenses. The audit covers all workspace crates with all features, including build and transitive dependencies, across the four release targets. Development dependencies are excluded.

`nix develop --command prek -a` runs the audit in local checks and CI. Nix builds generate the report offline using vendored dependencies. Release archives include `LICENSE` and `THIRD_PARTY_NOTICES.html`; Nix packages install both under `share/licenses/miniflow/`.

Generated workflow executables use the built-in bundle. When distributing them, include the applicable dependency notices; adding external plugins requires auditing their dependencies as well.
