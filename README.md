# miniflow

[![CI](https://github.com/linw1995/miniflow/actions/workflows/CI.yaml/badge.svg)](https://github.com/linw1995/miniflow/actions/workflows/CI.yaml)

miniflow is a Rust CLI project scaffold.

## Development

Enter the pinned development environment:

```sh
nix develop
```

Run the same checks as CI:

```sh
nix flake check -L
```

Build and run the CLI:

```sh
nix build .#miniflow
./result/bin/miniflow
```

## Releases

Push a `release/<version>` branch to build native archives for Linux and macOS.
The branch version must match `Cargo.toml`. The CD workflow publishes a GitHub
release tagged `v<version>` after all archives have been built.
