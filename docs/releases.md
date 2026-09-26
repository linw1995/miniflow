# Release prerequisites

A distributed CLI resolves exact-version `mf-runtime` and `mf-compiler` packages from crates.io. The bundled examples also require available `mfn-core` packages. Prepare and publish these support packages before publishing the matching CLI release.

## Verify packages through nextest

```sh
nix develop --command cargo nextest run -p mf-cli --test packaged_cli --test release_support
```

The Rust setup in `crates/mf-cli/tests/support/` packages the support crates, calculates archive checksums, and prepares
an isolated Cargo registry source. It builds a default CLI without development overrides. The generated registry runner
compiles all four extracted support packages outside the checkout, so separate package check builds are unnecessary.
Acceptance covers registry, pinned Git, and local-path nodes, locked rebuilds, and standalone execution. Temporary
artifacts are owned by the fixture and cleaned up after the test.

Release prerequisite tests invoke the Bash gate with a stub Cargo executable. They check the requested owners and exact-version manifest, and verify that Cargo resolution failures block release without contacting a registry or publishing packages. Both test targets are included in the standard nextest suite, coverage runs, and Nix checks.

## Publish support packages before the CLI

A maintainer must verify ownership of the package names, configure authorized registry credentials, and publish the matching versions in dependency order: runtime first, then compiler and built-in nodes. Package publication is a separate release action; the test fixture and release check never perform it.

After publication, check public availability and ownership:

```sh
bash scripts/check-release-support.sh --owner linw1995
```

This check lists registry owners and resolves the exact workspace version of every required package. Missing names, wrong ownership, unavailable versions, or resolution failures block the CLI release. The CD workflow runs it with the repository owner's login before building release archives. If ownership is intentionally transferred to a team, update that configured expectation explicitly.

The `release/<version>` branch must match the workspace version. Publishing an archive before its required support packages are available would leave users unable to compile Flows, even though `mf --help` works.
