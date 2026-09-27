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
| `builtin.if_else` | Nonempty ordered `branches` | None; activated by control edges | One boolean activation output per branch, plus `else` |

`builtin.code` is provided separately by `mfn-code`. It requires an explicit `language` field; the supported value is
`cel`. Input names have concrete type declarations, while output names and expressions live in `code`. The CEL checker
infers each output port type from its expression:

```json
{
  "language": "cel",
  "inputs": {"amount": "int"},
  "code": {"doubled": "amount * 2"}
}
```

The common input and output port contract is language-independent. A future backend can interpret its `code` payload
without changing existing CEL definitions. CEL input types include `int`, `double`, `bool`, `string`, `null`, and nested
`{"list": T}` or `{"map": T}` descriptors; maps have string keys and homogeneous values.

## Validation

The CLI checks node IDs, edge endpoints, selected output names, and cycles before generating runner code. The compiled runner validates registered kinds, configuration, ports, type compatibility, and required input connections before installation. Every failure returns a nonzero status and preserves an existing output executable.

Port connections are statically safe when the source type fits the target, such as `Int64` to `Number` or `List(Int64)` to
`Array`. A broad source can feed a refined target when the runtime checks the actual JSON value before invoking that
target: `Any` to `Int64`, `Number` to `Float64`, and `Array` to `List(Int64)` are examples. Concrete conflicts such as
`String` to `Int64` or `List(String)` to `List(Int64)` fail compilation. No values are coerced. Produced outputs are
checked against their declared types before publication, including outputs without consumers. A mismatch reports the
node, port, and nested JSON Pointer path where applicable.

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

## If, else-if, and else

The [single-condition example](../examples/if-else.json) returns `{"accepted":{"amount":150}}`. The [else-if example](../examples/else-if.json) returns `{"medium":{"amount":500}}`. Both use `audit` to trigger routing while conditions read the earlier `load_order.value` output from context. Incoming dependencies determine activation; condition references do not add dependencies.

Configure one or more ordered branches:

```json
{
  "branches": [
    {
      "id": "large",
      "condition": {
        "source": { "output": "load_order.value", "path": "/amount" },
        "operator": "gte",
        "value": 1000
      }
    },
    {
      "id": "medium",
      "condition": {
        "source": { "output": "load_order.value", "path": "/amount" },
        "operator": "gte",
        "value": 100
      }
    }
  ]
}
```

The first matching branch produces true; all other outputs are explicitly skipped. If no condition matches, `else` produces true. Later predicates are not evaluated after a match, but every predicate's syntax and output reference is validated during compilation. Branch IDs match `[A-Za-z_][A-Za-z0-9_-]*`, must be unique, and cannot be `else`. Changing array order changes precedence without changing port names.

`source.output` is an exact `${node_id}.${output_name}` context key. `source.path` is a JSON Pointer within that value: empty selects the root, `/items/0/price` selects a nested value, and `~0` / `~1` encode `~` / `/` in keys. Business values remain in context; router outputs are activation signals. Downstream nodes can bind the original data separately from their control dependencies.

| Operators | Literal | Behavior |
| --- | --- | --- |
| `eq`, `ne` | Required scalar `value` | Typed equality; strings are case-sensitive, numeric forms compare numerically |
| `gt`, `gte`, `lt`, `lte` | Required numeric `value` | Numeric ordering without rounding 64-bit integers through floating-point conversion |
| `exists`, `not_exists` | No `value` field | Test field or output availability |

No implicit conversion occurs. A present null exists and can be compared with an explicit null literal. Missing fields or explicitly skipped outputs are unavailable to existence checks; comparing them is an execution error. Unexpectedly omitted outputs and pending producers are errors even for existence checks. Objects and arrays cannot be compared in this version. Reached condition errors include the branch, qualified source, path, and operator.

A selected output can fan out to multiple downstream nodes; all eligible consumers execute. Only one branch output is active per router invocation. Execution is sequential in topological order, and a node gated by mutually exclusive outputs is skipped rather than acting as a merge. Compound boolean expressions and field-to-field comparisons are deferred.
