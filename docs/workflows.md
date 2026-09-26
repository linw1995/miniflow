# Workflow definitions

## JSON format

The [example definition](../examples/hello-workflow.json) uses schema version `2026-09-26`. This is the only accepted version at present. Each node has a unique, nonblank string `id`, a registered `kind`, and an optional JSON `config`. An edge connects a named output port to a named input port. An `outputs` entry selects a node port and gives it a name in the executable's JSON output.

## Node dependencies

The required `dependencies` object maps aliases to node packages. Aliases identify packages; workflow `kind` values are the names registered by those packages. A package can register several kinds. All declared packages participate in the build, and built-ins must be declared explicitly.

```json
{
  "dependencies": {
    "http": {
      "package": "example-http-nodes",
      "version": "1.2",
      "features": ["json"],
      "default-features": false
    },
    "local": {
      "package": "local-nodes",
      "path": "./nodes/local"
    }
  }
}
```

Each entry requires `package` and exactly one source: a crates.io `version`, a `git` URL with a full commit `rev`, or a local `path`. Features default to empty and default features are enabled. Unknown fields and incomplete or conflicting sources are rejected. The package names above are illustrative.

Paths resolve relative to the canonical definition's directory, including when the definition is accessed through a symlink. `order.json` uses `order.lock`; `order.flow.json` uses `order.flow.lock`. An extensionless definition has `.lock` appended. Different Flows can use independent dependencies in one directory. Compilation never rewrites the definition.

## Migrate older definitions

Definitions with version `2026-09-24` are rejected with a migration diagnostic. Change the version to `2026-09-26` and add `dependencies` declaring the packages that provide every node kind, including built-ins. An empty object is valid for definitions that require no plugins. Node configuration, connections, and selected outputs retain their meanings.

## Built-in nodes

Declare `mfn-core` once to use the basic built-in nodes below. To migrate older definitions, replace `mfn-constant` and `mfn-identity` dependencies with `mfn-core`, retaining node kinds and edges, then rebuild without `--locked` to update the adjacent lock. Subsequent builds can use `--locked` again.

| Kind | Configuration | Input ports | Output ports |
| --- | --- | --- | --- |
| `builtin.constant` | Required `value`: any JSON value | None | `value`: any value |
| `builtin.identity` | None | Required `input`: any value | `value`: the unchanged input |

## Validation

The CLI checks node IDs, edge endpoints, selected output names, and cycles before generating runner code. The compiled runner validates registered kinds, configuration, ports, type compatibility, and required input connections before installation. Every failure returns a nonzero status and preserves an existing output executable.

See [compiling workflows](compiling.md) to build and run a definition, or [plugin development](plugins.md) to add node kinds.

## Dependency locks

Commit the adjacent Flow lock with the definition. An unlocked build reuses compatible locked versions and updates resolution only when the current dependencies require it. `--locked` requires an existing compatible lock and never rewrites it. A cached Cargo lock is restored from the Flow lock before resolving dependencies; it is not an independent source of versions.

The lock records the runner and its validation dependencies. It does not freeze local path contents, native libraries, build-script inputs, or the Rust toolchain, and does not promise byte-identical executables.

Only one build can hold a Flow's dependency lock at a time. Contention reports a retry diagnostic. The adjacent guard file can remain after exit; the operating-system lock, not file presence, controls access. Failed compilation or validation leaves the prior Flow lock intact. Successful unlocked builds persist the lock atomically before installing the executable; installation failures report if the lock was already updated.

## Execution dependencies

The optional `control_edges` array contains entries with `from_node`, `from_output`, and `to_node`. A control edge establishes execution order without binding a target data input. Data and control edges together must be acyclic; repeated identical control edges are rejected. Existing `edges` retain their input binding and type rules.

Context references use qualified IDs such as `load_order.value`. A reference must name an output on an explicit predecessor, directly or transitively. Merely sorting earlier by node ID does not establish a dependency. Node IDs and local output names may contain dots if their combined IDs remain unique.

A produced control output activates its target regardless of whether its JSON value is false or null. All incoming data and control dependencies must be available; any explicit skip skips execution and propagates to downstream nodes. Unexpected missing outputs remain errors, including when another dependency is skipped. Independent nodes still execute, and connecting both mutually exclusive outputs to one node does not merge branches.

## Optional workflow results

A selection such as `{"name":"result","node":"branch","port":"value","optional":true}` omits its key when the source port or node is explicitly skipped. A produced null remains `"result": null`. Missing outputs without a skip marker remain errors. Output selections are required by default, so selecting a skipped required output fails with its name and source ID. All omitted selections produce `{}`.

The optional field requires updated support packages. Existing `2026-09-26` definitions retain strict defaults; default-false selections and empty control-edge arrays are omitted when serialized. Old runtimes reject new fields rather than silently changing behavior.
