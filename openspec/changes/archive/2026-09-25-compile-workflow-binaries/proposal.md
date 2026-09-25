# Proposal

## Why

The current `Flow` is only a runtime node container. It has no path for parsing and validating a workflow definition or producing an executable artifact. The project needs a defined plugin build model and a way to compile an embedded DAG definition into a standalone workflow binary.

## What Changes

- Define a workflow format for node kinds, node configuration, directed port connections, and selected outputs.
- Define plugin registration metadata, configuration factories, and port contracts; the compiler resolves node kinds from its linked plugin registry.
- Add a pipeline that validates definitions, builds an execution plan, generates a Rust runner, and compiles it into an executable binary with Cargo.
- Statically link plugin crates into generated workflow binaries. Runtime loading of arbitrary plugins is out of scope.

## Capabilities

### New Capabilities

- `workflow-binary-compilation`: Compile DAG workflow definitions with available plugins into standalone executable binaries.

### Modified Capabilities

## Impact

- The `mf` CLI and new runtime and compiler modules.
- Plugin registration APIs, node input/output port definitions, and the workflow definition format.
- Generated Cargo projects and `cargo build` artifacts; serialization and error handling dependencies may be added.
