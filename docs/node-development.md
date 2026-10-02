# Node development

## Implement a node

Basic nodes share the `mfn-core` crate under `crates/builtin-nodes/core/`. Nodes with distinct dependencies belong in separate packages.

The optional [`mfn-code`](../crates/builtin-nodes/code/) package registers `builtin.code` with CEL expressions,
instance-specific typed inputs, and output types inferred by the CEL checker. A Flow must select the package
explicitly; `mfn-core` does not register this kind. See the [CEL examples](workflows.md#built-in-nodes).

A plugin crate depends on `mf-runtime`, implements `Node::execute`, provides a factory, and submits a `NodeRegistration` through `inventory::submit!`. The registration declares a unique `kind` and its input and output `PortSpec` values. See [constant](../crates/builtin-nodes/core/src/constant.rs) and [identity](../crates/builtin-nodes/core/src/identity.rs) for working registrations.

`mfn-core` registers the `workflow.loop` declaration. The compiler replaces it with an engine-prepared subgraph executor, so a Flow using Loop must declare `mfn-core` in its top-level `dependencies`. `workflow.loop_assign`, `workflow.exit_loop`, and the synthetic `%loop` source remain reserved engine kinds. Ordinary nodes inside a Loop body also require their packages in `dependencies`.

## Registration contract

A plugin can register several unique kinds from one crate. Dependency aliases do not change these names. Linking must explicitly retain each plugin crate, for example with `extern crate plugin_alias as _;`; a `kind()` function is not required by the runtime API. The [external multi-node fixture](../crates/mf-compiler/tests/fixtures/multi-nodes/src/lib.rs) demonstrates this contract.

All plugins and the consumer must resolve the same `mf-runtime` package identity, including its version and source. Registrations from a different runtime identity are not entries in the consumer's inventory.

Factories validate configuration and construct instances during build validation and execution. Keep external I/O and business side effects in `Node::execute`; validation and `--describe` must not execute the workflow. Description mode reads the embedded graph without calling factories, so factory diagnostics cannot enter its JSON output. Building a Rust plugin can execute its build scripts and procedural macros with the build user's permissions.

## Select dependencies in a Flow

Declare plugin crates in the Flow's top-level `dependencies` object. The CLI generates imports for those packages and compiles a runner that validates and executes against the same registry. No predefined bundle or CLI rebuild is needed. See [workflow definitions](workflows.md) for registry, pinned Git, local-path, and feature syntax.

The CLI first checks graph structure, then builds the runner and invokes its `--validate` mode. Unknown kinds, duplicate registrations, invalid configuration, and incompatible ports fail before installation. The runner's normal mode executes generated node calls; validation never calls `Node::execute`.

The [multi-node fixture](../crates/mf-compiler/tests/fixtures/multi-nodes/) demonstrates one external crate registering several kinds. The packaged CLI acceptance script in [release prerequisites](releases.md) builds it from a packaged registry source outside the checkout.

See the [contribution guide](../CONTRIBUTING.md) for local checks and [dependency license auditing](licensing.md) before distributing additional plugins.

## Instance metadata

Nodes with configurable ports can override `Node::ports()` with `NodePorts` using `PortSpec::owned` for dynamic names; this replaces both static port lists for that instance. Ordinary registrations remain unchanged. Names must be nonempty and unique within each direction. Metadata must depend only on configuration.

The [Code node](../crates/builtin-nodes/code/src/lib.rs) uses this interface to expose ports from its declared inputs and checked CEL expressions.

Nodes whose outputs are fixed or directly copy an input can also override `Node::output_derivations()`. Return
`OutputDerivation::literal("value", value)` for a configured JSON value, or
`OutputDerivation::forward_input("value", "input")` when the output always equals that input. The default method returns
no derivations, so ordinary plugins retain their declared port types. Derivations must depend only on configuration and
must describe the actual result whenever the output is produced. Validation rejects references to undeclared ports,
duplicate output derivations, and literal values that conflict with the output's declared type. A plugin that advertises
an inaccurate derivation can cause an incorrect compile-time decision; runtime port checks still reject values outside
resolved types.

`ValueType::infer_json(&value)` derives a bounded port type for a configured JSON value. It uses refined scalar,
homogeneous list, and homogeneous map types when possible, and broad `Number`, `Array`, or `Object` where a precise
descriptor is unavailable. Known JSON values remain available as separate evidence, so a mixed array can still cause a
compile-time mismatch on a typed target. The [constant](../crates/builtin-nodes/core/src/constant.rs) and
[identity](../crates/builtin-nodes/core/src/identity.rs) nodes demonstrate both derivation forms.

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

`Inputs` and `Outputs` are standard `BTreeMap<String, ValueRef>` values. A `ValueRef` owns an
immutable JSON value; cloning it shares its payload and descendants. Convert JSON explicitly with
`.into()` when constructing outputs, for example `Outputs::from([("value".into(), json!(42).into())])`.
Read values through `as_*`, indexing, `pointer`, or `kind()`. `serde_json::to_value(&value)` creates
an owned JSON value when an external API requires one; serialization otherwise reads shared data directly.
Nodes forwarding an input should move or clone its handle. New arrays and objects can reuse child
handles with `ValueRef::array` and `ValueRef::object`. Context outputs remain available within their
execution scope.

History is opt-in: attach a `SnapshotRecorder` to an `ExecutionContext` when a consumer needs past
inputs and outputs. Ordinary execution retains current bindings without recording snapshots. The
recorder stores immutable global roots, shares unchanged branches, and interns equal values across
nodes and scopes. A repeated identical node state does not create a new snapshot. Loop passes and
Iteration items retain separate scope paths. Generated runners enable capture only when
`MF_CAPTURE_SNAPSHOTS=1` requests OTLP snapshot events. `SnapshotRecorder::with_sink` supports
other explicit consumers; records define each value once and reference its ID.
`--validate` and `--describe` do not capture data. Call `finish()` after an explicitly recorded run.

The executor calls `Node::execute_with_context_mut`, whose default implementation delegates to the read-only `execute_with_context` method. Loop assignment and exit use engine-owned operations behind this adapter; plugins cannot write Loop variables or publish outputs directly. A Loop body has its own output scope for every pass. A context reference inside that body resolves only within its scope, including the synthetic `%loop` outputs, and still requires an explicit ancestor dependency.

Keep output names local in node results. Runtime publication qualifies them with the instance ID. Explicit skipped names must be declared non-required outputs and cannot also be produced. A scheduler-skipped node propagates skipping through every output, including required ones. Context references alone never activate or skip a node.

Qualified output IDs must be unique. Both compiler validation and direct `Flow::new` construction reject collisions such as node `a.b` / output `c` and node `a` / output `b.c`, including optional outputs.

The shared executor checks every produced JSON value against its declared output type before publishing the node's result,
even when no downstream edge reads that port. It checks each bound input against its declared type before invoking an
active node. Nested list and map errors include a JSON Pointer path. A skipped node is not type-checked, and an
unexpectedly missing dependency remains an error before skip or type checks. Use `ValueType::Any` when a port legitimately
carries multiple JSON types; a specific declaration must match every produced value.

## Prepared execution nodes

`FlowNode::new` requires resolved `NodePorts`. Compiler preparation supplies these from the instance or static registration. Both in-memory execution and generated runners return `WorkflowRunError`. Context-aware implementations take `&ExecutionContext`; generated step helpers assume a validated plan and its execution order.

## Workflow observation context

Observed execution activates the workflow's OTel context and a node span around dependency resolution, invocation, and output publication. A node using an application-provided OTel tracer can create child spans through the current context without changing its execution interface. The runtime does not install a global provider or configure a plugin's tracer. Threads created by plugins require explicit context propagation.

Lifecycle events are emitted by the runtime independently of plugin diagnostic logs. Plugins continue to return values and explicit skipped ports normally; the runtime records success only after validating and publishing those results. See [observation contracts](observability.md) for provider ownership, failure phases, and a runnable SDK example.
