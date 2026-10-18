# Repository instructions

## Validation

- Use `nix develop` for the pinned toolchain.
- Run `prek install` to install the pre-commit and commit-msg hooks.
- Run `prek -a` before submitting changes; it covers formatting, Clippy, Cargo check, Markdown, workflows, and the license audit.
- Compile the shared planning Flow with `nix develop --command cargo run -p mf-cli --features development-support -- compile tools/test-affected/flows/test-affected.json --output target/test-affected-runner --build-dir target/test-affected-build --no-telemetry`.
- Identify affected tests with `nix develop --command cargo run -p mf-cli --features development-support -- run ./target/test-affected-runner --params.ci false --params.mode worktree --params.precision packages > target/test-plan.json`. Execute with `nix develop --command python3 tools/test-affected/run-tests.py target/test-plan.json`. The Flow owns selection; the script only consumes its plan.
- Use `--params.mode commit --params.head HEAD` for a single commit, or `--params.mode merge-base --params.base <local-base-ref> --params.head HEAD` for a branch comparison. `snapshot` compares supplied revisions directly. Local planning uses immutable snapshots and never fetches history; CI requires the actual clean tested merge. Local `--params.precision tests` enables semantic refinement.
- Apply the documented shared-input policy: package-owned manifests select their package and reverse dependencies; known repository configuration (including root dependencies) is deferred to full main/nightly runs; known shared executables select their consumers. Preserve full-workspace fallback for unclassified inputs and analyzer failures.
  An empty selection means no Rust package tests are needed; do not turn it into an unfiltered nextest invocation. Run any affected non-Rust checks separately. See [test selection](docs/ci.md#affected-package-tests) for details and limitations.
- PR CI runs the same Flow with its CI branch selected and compares against the nearest compatible successful tested merge snapshot, falling back to cumulative PR scope. Tests and the local incremental coverage gate use the same diff; only a completed successful workflow attempt with a valid record can advance the baseline. Main/nightly retain full suites and `rust` project coverage; PR reports remain scoped CI artifacts.
- For additional focused checks, use `nix develop --command cargo nextest run -p <crate>` or `-p <crate> --test <target>`. Do not run `nix flake check -L` locally: its `test` check reruns the complete suite that CI coverage already runs.
- When coverage is relevant, run `nix develop --command bash scripts/run-cov.sh`. Reports are written to `target/coverage/result/`, including `lcov.info`.

## Rust Visibility and Module Boundaries

- Decide the public surface at the highest relevant module entry point, through module declarations and exports. Follow the existing module structure and paths when extending the project.
- Prefer plain `pub` for shared symbols in modules exposed by those entry points. Keep module-local implementation details private.
- Use `pub(crate)`, `pub(super)`, and other scoped visibility sparingly, when a specific boundary requires it. Do not apply the narrowest possible visibility mechanically to every symbol. Existing scoped declarations are local decisions rather than a general restriction policy.
- Do not introduce facade modules, split implementation files, or relocate tests solely to restrict visibility or move scoped declarations elsewhere. Existing integration tests may call public planning and execution APIs; such calls alone do not justify making those APIs private.
- Before changing visibility, inspect the relevant code as it existed before the current change. Do not present an inferred preference as an established project rule. If the intended public surface remains unclear after checking instructions and existing code, confirm it with the user before changing it.

## Rust Error Handling

- Derive error types with `Snafu` and use generated selectors with `ResultExt::context` or `with_context` to add context to fallible operations. Avoid `map_err` closures that manually construct a Snafu variant.
- Use `#[snafu(transparent)]` for pass-through variants that add no useful context, so `?` can convert the source error directly.
- Preserve typed error sources and their chains. Do not stringify an error just to wrap it in another error; add a source-bearing variant and attach it with a Snafu selector.
- Use `ensure!` for checked invariants, `.fail()` for immediate domain errors, and `.build()` when an error value is needed without returning it directly.
- Expose or re-export context selectors only at module boundaries that need to attach that error context.
- Keep node-specific error types in their owning node crate. Runtime error types should describe shared runtime contracts and mechanics.

## Commits and pull requests

- Keep ablation reports local under the Git-ignored `target/` directory; do not include them in commits.
- Use `<type>(<scope>)[!]: <description>` for commit messages and PR titles.
- Allowed types: `build`, `chore`, `docs`, `feat`, `fix`, `perf`, `refactor`, `revert`, `style`, `test`.
- Allowed scopes: `cli`, `runtime`, `compiler`, `bundle`, `nodes`, `ci`, `deps`, `docs`.
- Use `deps` for dependency version updates.
- Use `chore(ci)` for CI configuration changes.
- Use `.github/pull_request_template.md` for every pull request and complete its AI Disclosure section.

## Releases

- A `release/<version>` branch must match the workspace version in the root `Cargo.toml`.
- The CD workflow builds Linux and macOS archives and publishes a GitHub release tagged `v<version>`.
