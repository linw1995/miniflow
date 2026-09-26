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

## Coverage

When coverage is relevant, run:

```sh
nix develop --command bash scripts/run-cov.sh
```

Reports are written to `target/coverage/result/`, including `lcov.info`.

## Code layout

| Path | Responsibility |
| --- | --- |
| `crates/mf-cli/` | The `mf` command and source workspace discovery |
| `crates/mf-compiler/` | Workflow validation, planning, code generation, and executable builds |
| `crates/mf-runtime/` | Workflow definitions, node interfaces, registry, and execution support |
| `crates/mf-bundle/` | Plugin selection shared by the compiler and generated runner |
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
