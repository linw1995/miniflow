# Tasks

## 1. Runtime contracts

- [x] 1.1 Define serializable `WorkflowDefinition`, node configuration, port-aware edges, and selected outputs; use a `YYYY-MM-DD` schema version and parsing examples to verify version handling and format diagnostics.
- [x] 1.2 Define `NodeRegistration`, port schemas, factories, and SNAFU-based error types; verify `inventory` registration and duplicate `kind` rejection with two test nodes.
- [x] 1.3 Refactor runtime `Flow` to retain node identities, connections, and a topological plan; verify port-based input routing with a hand-built DAG.
- [x] 1.4 Implement sequential execution and output collection; verify execution order and error locations with linear, branching, and node-failure scenarios.

## 2. Definition validation and compilation plan

- [x] 2.1 Resolve definitions to registered nodes and construct their configuration; verify unknown kinds and invalid configuration report the node ID.
- [x] 2.2 Validate node IDs, edge endpoints, port names and types, required inputs, and single-value connections; verify diagnostics with valid and invalid definitions.
- [x] 2.3 Implement cycle detection and deterministic topological sorting; verify rejection and stable ordering with cyclic graphs and graphs with multiple possible topological orders.
- [x] 2.4 Implement `CompiledWorkflow` and plan serialization/code generation; reparse the generated plan and verify semantic equivalence with the source definition.

## 3. Plugin bundle and executable generation

- [x] 3.1 Split out `mf-runtime` and `mf-compiler`, place each built-in node in its own crate under `crates/builtin-nodes`, and create a shared `mf-bundle` and separate `mf-cli`; read the registry from both compiler and runner and verify they expose the same `kind` set.
- [x] 3.2 Generate a Rust runner project from the workflow-specific source and per-node configuration artifacts; build it and verify direct node orchestration with the runtime and plugin bundle.
- [x] 3.3 Implement Cargo invocation, diagnostic forwarding, and atomic artifact writes for `mf compile <definition> --output <path>`; verify failed builds do not overwrite an existing target.
- [x] 3.4 Run a generated workflow binary and compare its outputs with the definition's selected outputs; verify the runner does not need the source definition file.

## 4. Documentation and integration

- [x] 4.1 Document plugin registration, plugin bundle selection, workflow definitions, and compilation commands; build and run the example from a clean target directory by following the documentation.
- [x] 4.2 Verify the end-to-end definition-to-executable flow; confirm unknown plugins, cycles, port errors, and Cargo build errors all produce nonzero exit statuses and actionable diagnostics.
