# Node development

## Implement a node

Basic nodes share the `mfn-core` crate under `crates/builtin-nodes/core/`. Nodes with distinct dependencies belong in separate packages.

The optional [`mfn-code`](../crates/builtin-nodes/code/) package registers `builtin.code` with CEL expressions,
instance-specific typed inputs, and output types inferred by the CEL checker. A Flow must select the package
explicitly; `mfn-core` does not register this kind. See the [CEL examples](workflows.md#built-in-nodes).

A plugin crate depends on `mf-runtime`, implements `Node::execute`, provides a factory, and submits a `NodeRegistration` through `inventory::submit!`. The registration declares a unique `kind` and its input and output `PortSpec` values. See [constant](../crates/builtin-nodes/core/src/constant.rs) and [identity](../crates/builtin-nodes/core/src/identity.rs) for working registrations.

## Registration contract

A plugin can register several unique kinds from one crate. Dependency aliases do not change these names. Linking must explicitly retain each plugin crate, for example with `extern crate plugin_alias as _;`; a `kind()` function is not required by the runtime API. The [external multi-node fixture](../crates/mf-compiler/tests/fixtures/multi-nodes/src/lib.rs) demonstrates this contract.

All plugins and the consumer must resolve the same `mf-runtime` package identity, including its version and source. Registrations from a different runtime identity are not entries in the consumer's inventory.

Factories validate configuration and construct instances during build validation and again during execution. Keep external I/O and business side effects in `Node::execute`; validation must not execute the workflow. Building a Rust plugin can execute its build scripts and procedural macros with the build user's permissions.

## Select dependencies in a Flow

Declare plugin crates in the Flow's top-level `dependencies` object. The CLI generates imports for those packages and compiles a runner that validates and executes against the same registry. No predefined bundle or CLI rebuild is needed. See [workflow definitions](workflows.md) for registry, pinned Git, local-path, and feature syntax.

The CLI first checks graph structure, then builds the runner and invokes its `--validate` mode. Unknown kinds, duplicate registrations, invalid configuration, and incompatible ports fail before installation. The runner's normal mode executes generated node calls; validation never calls `Node::execute`.

The [multi-node fixture](../crates/mf-compiler/tests/fixtures/multi-nodes/) demonstrates one external crate registering several kinds. The packaged CLI acceptance script in [release prerequisites](releases.md) builds it from a packaged registry source outside the checkout.

See the [contribution guide](../CONTRIBUTING.md) for local checks and [dependency license auditing](licensing.md) before distributing additional plugins.

## Instance metadata

Nodes with configurable ports can override `Node::ports()` with `NodePorts` using `PortSpec::owned` for dynamic names; this replaces both static port lists for that instance. Ordinary registrations remain unchanged. Names must be nonempty and unique within each direction. Metadata must depend only on configuration.

The [Code node](../crates/builtin-nodes/code/src/lib.rs) uses this interface to expose ports from its declared inputs and checked CEL expressions.

Port types include the broad JSON categories `Any`, `Null`, `Boolean`, `Number`, `String`, `Array`, and `Object`, plus `Int64`, `Float64`, and recursive `List(T)` and `Map(T)`. `Map(T)` describes an object with string keys and values of type `T`. Existing `PortSpec::new` registrations remain valid for static broad or scalar ports. Construct typed collection ports from `Node::ports()`:

```rust
let items = ValueType::List(Box::new(ValueType::Int64));
let input = PortSpec::owned("items", items, true);
```

Recursive types make `ValueType` cloneable but no longer `Copy`. Rust callers that previously moved a type from a borrowed port descriptor must borrow it or call `.clone()`.

Connection validation accepts statically safe widening and runtime-checked narrowing from broad or `Any` sources. For
example, an `Any` output can feed an `Int64` input; the consumer runs only if the actual JSON value is a signed integer.
Concrete conflicts such as `String` to `Int64` are rejected during compilation. Direct callers matching
`WorkflowCompileError::IncompatiblePortTypes` now receive boxed `ValueType` fields and can dereference or clone them.
Rebuild plugins against the matching runtime package and correct declarations that do not describe their produced values.

Declare context reads with `Node::context_references()`. Each `ContextReference` contains a qualified output ID and a diagnostic label, such as a branch ID. Validation resolves exact `${node_id}.${output_name}` keys and requires the producer to be a strict ancestor through explicit dependencies. References do not add edges. Ambiguous qualified IDs are rejected with both source pairs.

## Context-aware execution

Override `Node::execute_with_context` to read declared outputs through `ctx.output("source.value")` and return
`NodeResult`. The default adapter calls ordinary `execute` once. `ContextValue` distinguishes a produced JSON value from
`Skipped`; unavailable outputs, including reads before production and unexpected omissions, are errors. Reference
declarations support compile-time dependency checks; the context does not enforce a runtime read whitelist. The runtime
publishes results only after successful execution and starts with fresh context for each run.

Keep output names local in node results. Runtime publication qualifies them with the instance ID. Explicit skipped names must be declared non-required outputs and cannot also be produced. A scheduler-skipped node propagates skipping through every output, including required ones. Context references alone never activate or skip a node.

The shared executor checks every produced JSON value against its declared output type before publishing the node's result,
even when no downstream edge reads that port. It checks each bound input against its declared type before invoking an
active node. Nested list and map errors include a JSON Pointer path. A skipped node is not type-checked, and an
unexpectedly missing dependency remains an error before skip or type checks. Use `ValueType::Any` when a port legitimately
carries multiple JSON types; a specific declaration must match every produced value.

## Prepared execution nodes

`FlowNode::new` requires resolved `NodePorts`. Compiler preparation supplies these from the instance or static registration. Both in-memory execution and generated runners return `WorkflowRunError`. Context-aware implementations take `&ExecutionContext`; generated step helpers assume a validated plan and its execution order.
