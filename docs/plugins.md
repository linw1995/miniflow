# Plugin development

## Implement a node

Each built-in node lives in its own crate under `crates/builtin-nodes/`. A plugin crate depends on `mf-runtime`, implements `Node::execute`, provides a factory, and submits a `NodeRegistration` through `inventory::submit!`. The registration declares a unique `kind` and its input and output `PortSpec` values. See [constant](../crates/builtin-nodes/constant/src/lib.rs) and [identity](../crates/builtin-nodes/identity/src/lib.rs) for working registrations.

## Registration contract

A plugin can register several unique kinds from one crate. Dependency aliases do not change these names. Linking must explicitly retain each plugin crate, for example with `extern crate plugin_alias as _;`; a `kind()` function is not required by the runtime API. The [external multi-node fixture](../crates/mf-compiler/tests/fixtures/multi-nodes/src/lib.rs) demonstrates this contract.

All plugins and the consumer must resolve the same `mf-runtime` package identity, including its version and source. Registrations from a different runtime identity are not entries in the consumer's inventory.

Factories validate configuration and construct instances during build validation and again during execution. Keep external I/O and business side effects in `Node::execute`; validation must not execute the workflow. Building a Rust plugin can execute its build scripts and procedural macros with the build user's permissions.

## Select the bundle

`inventory` only collects registrations from linked crates. Workspace membership alone does not link a plugin into the compiler or generated executable.

The current plugin set is selected by [mf-bundle](../crates/mf-bundle/Cargo.toml): add the node crate as a dependency there and reference its exported `kind()` in [mf-bundle's registry](../crates/mf-bundle/src/lib.rs). Rebuild `mf` after changing the bundle. Both the compiler and generated runner use this bundle, so they see the same node kinds. Bundle selection is currently a build-time choice; the CLI has no bundle flag.

See the [contribution guide](../CONTRIBUTING.md) for local checks and [dependency license auditing](licensing.md) before distributing additional plugins.
