# Dependency license audit

Generate the dependency license report with the pinned development environment:

```sh
nix develop --command bash scripts/generate-third-party-notices.sh
```

The report is written to `target/THIRD_PARTY_NOTICES.html`. `cargo-about` checks the license allowlist in [about.toml](../about.toml) and fails on unaccepted or unresolved licenses. The audit covers all workspace crates with all features, including build and transitive dependencies, across the three configured release targets (Apple Silicon macOS and x86_64/ARM64 Linux). Development dependencies are excluded.

`nix develop --command prek -a` runs the audit locally. In CI, the Nix package check generates the report offline using vendored dependencies and fails on unaccepted or unresolved licenses. Release archives include `LICENSE` and `THIRD_PARTY_NOTICES.html`; Nix packages install both under `share/licenses/miniflow/`.
The workspace audit includes the optional `mfn-code` crate and its CEL evaluator dependency, `cel-core`.
It also includes the optional `mfn-llm` crate, Rig provider contracts and HTTP transport, and their transitive dependencies.
The optional `mfn-deepseek` crate adds its reqwest JSON transport and Tokio reactor support to the same audit.
The `mf-chat` core adds Diesel, embedded migrations, database ownership locking, and bundled SQLite to the workspace audit.

Generated workflow executables link the node packages selected by their Flow definition and the support packages used for validation and execution. Audit the generated Cargo project retained in the build directory and include the applicable dependency notices when distributing its executable. The repository audit alone does not cover third-party Nodes selected by another Flow.

## Desktop dependency and resource review

The Ely desktop feasibility review approved these **package-scoped** additions in `about.toml`:

| Package | Reviewed version | Additional accepted license | Reason |
| --- | --- | --- | --- |
| `cbindgen` | 0.28.0 | MPL-2.0 | GPUI's Apple platform build dependency |
| `option-ext` | 0.2.0 | MPL-2.0 | Font discovery and Ely's macOS WebView dependency |
| `libbz2-rs-sys` | 0.2.6 | bzip2-1.0.6 | GPUI HTTP compression support |
| `ely-gpui-component` | Git `5d6efd46ec0c93b381ec7578561a1273c366025b` | OFL-1.1 | Embedded font resources, not Ely's MIT-licensed code |

These licenses are not added to the global allowlist. `cargo-about` scopes additions by package name, not version;
review the licenses and source obligations again when updating the listed packages. Build and transitive dependency
coverage remains enabled. Neither these additions nor the resource clarification changes miniflow's Apache-2.0 license
or approves GPL dependencies.

When distributing MPL-covered code, preserve its notices and license and satisfy the MPL's source-availability
requirements for covered files and any modifications. A build dependency being audited does not, by itself, mean
that its code is part of a distributed executable. Preserve the bzip2 license and attribution for distributed code;
the generated report includes the corresponding source license text.

### Embedded Ely assets

Ely's crate metadata names MIT, but its asset bundle also includes three independently attributed fonts and Lucide
icons, some derived from Feather. The checksum-verified `[ely-gpui-component.clarify]` entry accounts for all of them:

| Resource | Copyright / attribution | License and source file |
| --- | --- | --- |
| Inter | 2016 The Inter Project Authors | OFL-1.1, `assets/fonts/Inter-LICENSE.txt` |
| JetBrains Mono | 2020 The JetBrains Mono Project Authors | OFL-1.1, `assets/fonts/JetBrainsMono-LICENSE.txt` |
| IBM Plex Sans | 2017 IBM Corp.; Reserved Font Name "Plex" | OFL-1.1, `assets/fonts/ibm-plex-sans/IBMPlexSans-LICENSE.txt` |
| Lucide icons | 2026 Lucide Icons and Contributors | ISC, first section of `assets/icons/LICENSE` |
| Feather-derived icons | 2013-present Cole Bemis; original derived-icon list retained | MIT, second section of `assets/icons/LICENSE` |

The clarification includes Ely's own MIT `LICENSE` as well. It preserves the three original font notices separately,
and splits the icon file into its ISC and MIT sections without dropping the Feather-derived icon list. Reports show
source paths beside the complete texts. These are local files from the resolved dependency, so the same process works
offline with vendored sources; no hand-copied or canonical-only font notice replaces the original attribution.

Desktop distributions must ship `THIRD_PARTY_NOTICES.html` containing those texts whenever they embed Ely's assets,
including copyright statements and reserved font names. Keep the fonts under OFL-1.1, do not sell them by themselves,
and review reserved-name restrictions before distributing modified fonts. Keep both the Lucide and Feather notices,
not only the ISC heading. Gallery-only fonts and images are outside the application's `assets` bundle and this review.

`cargo-about 0.9.0` can warn and fall back to crate metadata when a clarification file is missing or its checksum
changes, even with `--fail`. The notice generator explicitly retains warnings and rejects that fallback before
publishing a report. Re-review changed files and update their checksums intentionally; do not disable clarification
checks to restore a successful build. Offline fixtures test notice retention, checksum drift, and missing resources:

```sh
nix develop --command python3 -m unittest discover -s scripts -p test_license_notices.py
```

Until desktop is integrated, the workspace report correctly contains no Ely notices because Ely is absent from its
dependency graph. The isolated feasibility probe can exercise the approved policy and the full original-resource report:

```sh
CARGO_ABOUT_OFFLINE=1 nix develop --command bash scripts/generate-third-party-notices.sh \
  --manifest-path target/desktop-probe/Cargo.toml target/desktop-probe/THIRD_PARTY_NOTICES.html
```

`--manifest-path` selects an explicit Cargo project while retaining the repository's policy and template. It is
mutually exclusive with `--release`, which retains the existing CLI-only release scope. Without either option the
script audits the full workspace.

The existing release archive and Nix package paths already include the generated HTML. After adding the optional
desktop dependency, re-run the full workspace and release-scope audits and verify the actual archives include the
resource texts; this review does not claim that desktop packaging or platform acceptance is complete.
