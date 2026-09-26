# Repository instructions

## Validation

- Use `nix develop` for the pinned toolchain.
- Run `prek install` to install the pre-commit and commit-msg hooks.
- Run `prek -a` and `nix flake check -L` before submitting changes.
- When coverage is relevant, run `nix develop --command bash scripts/run-cov.sh`. Reports are written to `target/coverage/result/`, including `lcov.info`.

## Commits and pull requests

- Use `<type>(<scope>)[!]: <description>` for commit messages and PR titles.
- Allowed types: `build`, `chore`, `ci`, `docs`, `feat`, `fix`, `perf`, `refactor`, `revert`, `style`, `test`.
- Allowed scopes: `cli`, `runtime`, `compiler`, `bundle`, `nodes`, `ci`, `docs`.
- Use `.github/pull_request_template.md` for every pull request and complete its AI Disclosure section.

## Releases

- A `release/<version>` branch must match the workspace version in the root `Cargo.toml`.
- The CD workflow builds Linux and macOS archives and publishes a GitHub release tagged `v<version>`.
