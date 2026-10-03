# Repository instructions

## Validation

- Use `nix develop` for the pinned toolchain.
- Run `prek install` to install the pre-commit and commit-msg hooks.
- Run `prek -a` and `nix flake check -L` before submitting changes.
- When coverage is relevant, run `nix develop --command bash scripts/run-cov.sh`. Reports are written to `target/coverage/result/`, including `lcov.info`.

## Rust Visibility and Module Boundaries

- Decide the public surface at the highest relevant module entry point, through module declarations and exports. Follow the existing module structure and paths when extending the project.
- Prefer plain `pub` for shared symbols in modules exposed by those entry points. Keep module-local implementation details private.
- Use `pub(crate)`, `pub(super)`, and other scoped visibility sparingly, when a specific boundary requires it. Do not apply the narrowest possible visibility mechanically to every symbol. Existing scoped declarations are local decisions rather than a general restriction policy.
- Do not introduce facade modules, split implementation files, or relocate tests solely to restrict visibility or move scoped declarations elsewhere. Existing integration tests may call public planning and execution APIs; such calls alone do not justify making those APIs private.
- Before changing visibility, inspect the relevant code as it existed before the current change. Do not present an inferred preference as an established project rule. If the intended public surface remains unclear after checking instructions and existing code, confirm it with the user before changing it.

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
