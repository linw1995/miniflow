# Design

## Context

See proposal.md - Why. The repository currently has only a `Node::execute(Inputs)` sketch and a runtime `Flow` represented by `Vec<Vec<usize>>`. `inventory` collects registrations linked into the current program; it does not discover Cargo crates from a workflow file.

## Goals / Non-Goals

**Goals:**

- Produce a validated, fixed workflow execution plan from a declarative DAG definition.
- Generate Rust orchestration code that directly executes nodes in topological order and routes named port values.
- Link node plugins into the final standalone binary and provide actionable build diagnostics.
- Initially support single-process sequential topological execution, leaving room for later parallel scheduling.

**Non-Goals:**

- Load arbitrary Rust dynamic libraries into a running workflow process.
- Generate plugin implementation code or eliminate `Node` trait dispatch.
- Distributed execution, parallel scheduling, hot updates, and cross-version plugin ABIs.

## Decisions

### 1. Statically link plugins through Cargo dependencies

Plugin crates implement the runtime's `Node` trait and use `inventory::submit!` to submit a registration containing the `kind`, port contracts, and factory. The compiler and generated runner use the same explicit plugin bundle, initially a crate containing built-in nodes. Cargo features or bundle dependencies can select plugin sets later. The compiler only accepts node kinds visible in its linked set.

Static dependencies are chosen because Rust trait objects are not a stable cross-dynamic-library ABI. `inventory` registrations are evaluated for linked images or when a dynamic library is loaded; this does not provide plugin package management. Runtime dylib and ABI plugins require a separate design.

### 2. Separate definitions, compiled plans, and runtime Flow

- `WorkflowDefinition`: Serializable JSON input containing a schema version represented by a closed Rust enum and serialized as a `YYYY-MM-DD` string (initially `2026-09-24`), nodes (`id`, `kind`, `config`), port-aware edges, and selected outputs. Node references use `DefinitionId`, serialized as strings. Deserialization rejects every version not explicitly supported by the enum.
- `CompiledWorkflow`: Stable intermediate plan generated after resolving plugins and validating structure. Nodes follow deterministic topological order; edges and selected outputs use stable sorted order. Kahn's algorithm selects the lowest definition ID among ready nodes; a residual graph with no ready nodes produces a cycle path containing only cycle members.
- `Flow`: In-process library API containing instantiated `Box<dyn Node>` values, port connection mappings, and an execution plan. Generated workflow binaries execute emitted Rust orchestration code and do not instantiate `Flow`.

Edges represent `from_node/from_output -> to_node/to_input`. Initially, a single-value input accepts one connection. A specific output type is assignable to the same input type or `Any`; an `Any` output is only assignable to an `Any` input. Multiple sources and merge rules are deferred until explicitly defined, avoiding implicit overwrites.

`Flow::new` receives definition IDs, resolves them to internal `NodeId` indices, and validates unique IDs, connections, selected outputs, and the supplied topological order before returning a runnable flow. A caller cannot supply arbitrary runtime indices to this constructor. Execution errors are limited to node failures and values that nodes fail to produce.

### 3. Generate a Rust runner and compile it with Cargo

`mf compile <definition> --output <path>` parses the definition, resolves the registry, and validates IDs, node kinds, configuration, ports, and cycles.
It produces a normalized `CompiledWorkflow`, generates Rust code with `quote` tokens and `syn` string literals, formats the parsed syntax tree with `prettyplease`, and writes per-node JSON configuration files. It then creates a temporary Cargo project that depends on the runtime and plugin bundle.
The generated project contains `src/main.rs`, `src/workflow.rs`, `src/config_*.json`, and a reviewable `workflow-plan.json`; its binary prints selected outputs as JSON. The binary embeds node configuration and runs without the source definition or generated project. Its manifest isolates the project from a containing workspace and uses absolute paths for the runtime and bundle dependencies.
The CLI locates the local runtime and bundle crates from the definition path or current directory, so this initial path-dependency build requires a source checkout. It streams diagnostics from `cargo build --release`, stages the executable in the output directory, and atomically renames it to the target path. Failed Cargo builds retain the generated project for inspection and never replace an existing target. The normalized plan remains a reviewable build artifact but is not loaded by the executable.

The generated runner uses the linked plugin registry to find node factories, then follows statically emitted statements for node execution, input routing, and selected output collection. The workflow graph is needed while compiling; the executable does not use a graph scheduler. Node implementations are still invoked through the shared `Node` trait. Direct plugin function references would require an additional code-generation contract from plugins and are deferred.

### 4. Use runtime values with static port metadata validation

Node registration metadata declares input/output port names, types, and requiredness. `Node::execute` consumes `Inputs` indexed by port name and returns `Outputs`; initially, serializable values carry data between nodes. Port names and types are validated at compile time. Generated runner helpers check that nodes actually produce values needed by downstream connections and selected outputs, reporting definition IDs and port names.

Domain errors across the project use SNAFU-derived error types organized by stage: `DefinitionParseError` for JSON parsing; `WorkflowCompileError`, `NodeBuildError`, and `FlowBuildError` for compilation and construction; and `NodeExecutionError` and `FlowExecutionError` for execution. Underlying errors are preserved as sources and enriched with context at each boundary. This keeps invalid definition references out of runtime error paths while allowing the CLI to show useful context.

### 5. Use crate boundaries to support artifact builds

`mf-runtime` owns shared definition types, node contracts, registration, in-process Flow execution, and generated-runner helpers. `mf-compiler` owns validation and code generation. `mf-cli` owns the `mf` binary and its command handling.
`crates/builtin-nodes` is a container directory, with one `mf-node-*` crate per built-in node. `mf-bundle` anchors those crates and verifies their kinds are visible through `inventory`. Both the compiler and generated runner depend on that bundle.
Initially, the generated project depends on workspace path versions; crates.io or vendored offline distribution can be defined later.

## Risks / Trade-offs

- [Compiler and runner plugin sets diverge] → Share a plugin bundle and verify required node kinds during compilation.
- [Cargo sub-builds increase compile time and require dependency downloads] → Preserve generated projects and complete diagnostics on build failures; define safe cache sharing when concurrent builds are supported.
- [Some configuration or port types are only knowable at runtime] → Provide statically verifiable schemas in registrations and explicitly mark checks that must remain dynamic.
- [Plugin factories run during compilation and again when the runner starts] → Keep factories limited to local configuration parsing and node construction; perform external I/O in `Node::execute`.
- [Binaries are not portable across platforms] → Make the compile target explicit; defer cross-compilation support.
- [Generated-code injection or path handling errors] → Construct definition strings as `syn::LitStr` tokens, parse generated tokens before formatting them, serialize node configurations into separate JSON files, and isolate temporary build directories.

## Migration Plan

1. Implement definition, plugin registration, validation, and runtime execution APIs while retaining direct Flow construction for library users.
2. Add the compile CLI and Cargo runner project to produce runnable standalone binaries.
3. Document a minimal built-in plugin and workflow example; do not overwrite an existing target when a build fails.

To roll back, remove the CLI compilation entry point and generated project support while retaining the runtime Flow API as a library capability. No persisted data migration is needed because no definition format exists yet.
