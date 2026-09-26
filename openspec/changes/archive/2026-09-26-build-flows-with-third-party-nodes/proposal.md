# Proposal

## Why

Users need to build Flows with third-party Nodes using an installed CLI and project files. Today, adding a node requires editing `mf-bundle`, rebuilding the CLI, and retaining a miniflow checkout, so the available nodes are controlled by the CLI distribution rather than the Flow project.

## What Changes

- Add a required top-level `dependencies` object to workflow JSON schema `2026-09-26`, declaring node crates from crates.io, Git revisions, or local paths, with feature selection.
- Generate the plugin aggregation code from project dependencies during each build; no predefined bundle or CLI rebuild is required.
- Generate one runner with a validation mode; perform structural checks in the CLI and plugin-dependent checks in the compiled runner before installing it.
- Reuse generated build directories and Cargo artifacts across invocations, with automatic directory selection and an optional `--build-dir` override.
- Persist dependency resolution in a per-definition lock file (`flow.json` -> `flow.lock`) and support `--locked` builds.
- Resolve the matching runtime and compiler packages independently of a source checkout; prepare their release packaging and availability alongside built-in node crates.
- **BREAKING**: Replace workflow schema `2026-09-24` with `2026-09-26` and require explicit dependency declarations, including built-in nodes. Remove the implicit built-in registry from the production compiler and CLI dependency graph. Preserve graph semantics and the node execution contract.

## Capabilities

### New Capabilities

- `flow-node-dependencies`: Declare, resolve, and lock project-owned node dependencies, with consistent build inputs and actionable dependency diagnostics.

### Modified Capabilities

- `workflow-binary-compilation`: Compile with project-selected plugins through a generated runner with validation and execution modes, without a CLI rebuild or miniflow checkout, while retaining standalone output and failure guarantees.

## Impact

- `mf-cli`: embedded dependency parsing, dependency resolution, runner validation orchestration, reusable build-directory lifecycle, lock handling, and build diagnostics.
- `mf-compiler`: remove its production dependency on the fixed bundle; reuse registry-parameterized validation and code generation.
- `mf-runtime` and node crates: extend the versioned definition model while retaining execution and registration contracts; document link retention and shared runtime compatibility.
- `mf-bundle`: remove from production builds; migrate existing fixtures and examples to generated aggregation.
- Cargo package metadata, release preparation, CLI integration tests, and workflow/plugin documentation.
- Users still need Cargo, a compatible Rust toolchain, and any native dependencies required by their nodes.
