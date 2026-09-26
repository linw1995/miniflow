# Plugin development

## Implement a node

Basic nodes share the `mfn-core` crate under `crates/builtin-nodes/core/`. Nodes with service-specific dependencies belong in separate packages.

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

Nodes with configurable ports can override `Node::ports()` with owned `NodePorts`; this replaces both static port lists for that instance. Ordinary registrations remain unchanged. Names must be nonempty and unique within each direction. Metadata must depend only on configuration.

Declare context reads with `Node::context_references()`. Each `ContextReference` contains a qualified output ID and a diagnostic label, such as a branch ID. Validation resolves exact `${node_id}.${output_name}` keys and requires the producer to be a strict ancestor through explicit dependencies. References do not add edges. Ambiguous qualified IDs are rejected with both source pairs.
