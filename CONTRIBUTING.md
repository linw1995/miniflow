# Contributing

## Environment and checks

Use the pinned toolchain from the repository root and install the pre-commit and commit-msg hooks:

```sh
nix develop
prek install
```

Before submitting changes, run:

```sh
prek -a
nix flake check -L
```

These checks cover formatting, Rust diagnostics, tests, workflow configuration, Markdown, and the [dependency license audit](docs/licensing.md).

## Tests

The standard nextest suite includes packaged CLI acceptance and release prerequisite checks:

```sh
nix develop --command cargo fetch --locked
nix develop --command cargo nextest run --workspace --all-targets --all-features
```

Fetch the complete locked dependency graph before a first test run: offline fixture builds also resolve dependencies for other targets. The coverage and release workflows perform this preparation automatically.

To run only these integration tests:

```sh
nix develop --command cargo nextest run -p mf-cli --test packaged_cli
```

`packaged_cli_acceptance` builds a default CLI and third-party packages, then verifies registry, pinned local Git, and local-path dependencies outside the checkout, locked rebuilds, and standalone execution. It is enabled without a feature flag or `--ignored`. Nextest captures its subprocess diagnostics and reserves the test worker slots while it runs to avoid competing nested Cargo builds. Python and Git are included in the development shell.

Nix uses the same nextest suite. Package builds leave test execution to that check so acceptance runs once. The coverage command below also discovers these tests through nextest; there is no separate post-check invocation to maintain.

## Coverage

When coverage is relevant, run:

```sh
nix develop --command bash scripts/run-cov.sh
```

Reports are written to `target/coverage/result/`, including `lcov.info`.

## Code layout

| Path | Responsibility |
| --- | --- |
| `crates/mf-cli/` | The `mf` command and project build orchestration |
| `crates/mf-compiler/` | Workflow validation, planning, code generation, and executable builds |
| `crates/mf-runtime/` | Workflow definitions, node interfaces, registry, and execution support |
| `crates/builtin-nodes/` | Built-in node implementations |

See [plugin development](docs/plugins.md) for adding nodes.

## Commits and pull requests

Use `<type>(<scope>)[!]: <description>` for commit messages and pull request titles. The optional `!` marks a breaking change.

- Types: `build`, `chore`, `ci`, `docs`, `feat`, `fix`, `perf`, `refactor`, `revert`, `style`, `test`.
- Scopes: `cli`, `runtime`, `compiler`, `bundle`, `nodes`, `ci`, `docs`.

For example:

```text
docs(docs): simplify documentation navigation
```

Keep pull request descriptions concise and explain the problem and resulting behavior.
