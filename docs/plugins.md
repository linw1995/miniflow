# Plugin development

## Implement a node

Each built-in node lives in its own crate under `crates/builtin-nodes/`. A plugin crate depends on `mf-runtime`, implements `Node::execute`, provides a factory, and submits a `NodeRegistration` through `inventory::submit!`. The registration declares a unique `kind` and its input and output `PortSpec` values. See [constant](../crates/builtin-nodes/constant/src/lib.rs) and [identity](../crates/builtin-nodes/identity/src/lib.rs) for working registrations.

## Select the bundle

`inventory` only collects registrations from linked crates. Workspace membership alone does not link a plugin into the compiler or generated executable.

The current plugin set is selected by [mf-bundle](../crates/mf-bundle/Cargo.toml): add the node crate as a dependency there and reference its exported `kind()` in [mf-bundle's registry](../crates/mf-bundle/src/lib.rs). Rebuild `mf` after changing the bundle. Both the compiler and generated runner use this bundle, so they see the same node kinds. Bundle selection is currently a build-time choice; the CLI has no bundle flag.

See the [contribution guide](../CONTRIBUTING.md) for local checks and [dependency license auditing](licensing.md) before distributing additional plugins.
