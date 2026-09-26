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

The built-in bundle currently provides:

| Kind | Configuration | Input ports | Output ports |
| --- | --- | --- | --- |
| `builtin.constant` | Required `value`: any JSON value | None | `value`: any value |
| `builtin.identity` | None | Required `input`: any value | `value`: the unchanged input |

## Validation

The compiler rejects unknown kinds, invalid configuration, missing nodes or ports, incompatible port types, missing required inputs, multiply connected inputs, repeated output names, and cycles before generating a runner.

See [compiling workflows](compiling.md) to build and run a definition, or [plugin development](plugins.md) to add node kinds.
