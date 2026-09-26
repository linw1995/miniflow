# Workflow definitions

## JSON format

The [example definition](../examples/hello-workflow.json) uses schema version `2026-09-24`. This is the only accepted version at present. Each node has a unique, nonblank string `id`, a registered `kind`, and an optional JSON `config`. An edge connects a named output port to a named input port. An `outputs` entry selects a node port and gives it a name in the executable's JSON output.

## Built-in nodes

The built-in bundle currently provides:

| Kind | Configuration | Input ports | Output ports |
| --- | --- | --- | --- |
| `builtin.constant` | Required `value`: any JSON value | None | `value`: any value |
| `builtin.identity` | None | Required `input`: any value | `value`: the unchanged input |

## Validation

The compiler rejects unknown kinds, invalid configuration, missing nodes or ports, incompatible port types, missing required inputs, multiply connected inputs, repeated output names, and cycles before generating a runner.

See [compiling workflows](compiling.md) to build and run a definition, or [plugin development](plugins.md) to add node kinds.
