# Workflow definitions

## JSON format

The [basic example](../examples/hello-workflow.json) uses schema version `2026-09-26` for a flat DAG. Version `2026-09-29` adds structured Loop bodies and is required when using Loop control kinds. Both versions are accepted. Each ordinary node has a unique, nonblank string `id`, a registered `kind`, and an optional JSON `config`. An edge connects a named output port to a named input port. An `outputs` entry selects a node port and gives it a name in the executable's JSON output.

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

Built-in nodes and subgraph declarations are provided by node packages. Include `mfn-core` for the
Loop declaration; its assignment and exit controls are supplied by the engine. For example,
[the CEL scalar Flow](../examples/cel-scalar.json) declares both packages it uses (these paths are
relative to the definition in `examples/`):

```json
{
  "dependencies": {
    "core": { "package": "mfn-core", "path": "../crates/builtin-nodes/core" },
    "code": { "package": "mfn-code", "path": "../crates/builtin-nodes/code" }
  }
}
```

`mfn-core` provides the constant feeding the expression; `mfn-code` registers `builtin.code`. A Flow using only `builtin.code` needs only `mfn-code`. Replace local paths with available registry versions or pinned Git sources when compiling outside this checkout.

Paths resolve relative to the canonical definition's directory, including when the definition is accessed through a symlink. `order.json` uses `order.lock`; `order.flow.json` uses `order.flow.lock`. An extensionless definition has `.lock` appended. Different Flows can use independent dependencies in one directory. Compilation never rewrites the definition.

## Migrate older definitions

Definitions with version `2026-09-24` are rejected with a migration diagnostic. Change the version to `2026-09-26` and add `dependencies` declaring the packages that provide every node kind, including built-ins. An empty object is valid for definitions that require no plugins. Node configuration, connections, and selected outputs retain their meanings.

Built-in body sources use the `%` prefix: `%loop` for Loop variables and `%iteration` for Iteration
items. In earlier definitions, replace `$loop` with `%loop` and `@iteration` with `%iteration` in
edge endpoints, body result selections, and context output references before recompiling. For
example, `$loop.count` becomes `%loop.count`, and `@iteration.items` becomes `%iteration.item`.

## Built-in nodes

Declare the package for each built-in kind you use. `mfn-core` provides the basic nodes; `mfn-code` provides the optional CEL Code node. To migrate older definitions, replace `mfn-constant` and `mfn-identity` dependencies with `mfn-core`, retaining node kinds and edges, then rebuild without `--locked` to update the adjacent lock. Subsequent builds can use `--locked` again.

| Package | Kind | Configuration | Input ports | Output ports |
| --- | --- | --- | --- | --- |
| `mfn-core` | `builtin.constant` | Required `value`: any JSON value | None | `value`: inferred from the configured value |
| `mfn-core` | `builtin.identity` | None | Required `input`: any value | `value`: the unchanged input, with its known type |
| `mfn-core` | `builtin.if_else` | Nonempty ordered `branches` | None; activated by control edges | One boolean activation output per branch, plus `else` |
| `mfn-core` | `builtin.iteration` | Body graph, mode, and item error policy | Required `items`: array or object | `results`: collected array |
| `mfn-core` | [`builtin.batch`](#batch-collection) | Positive `max_items` and `max_wait_ms`; streaming mode | Required `item`: `T` | `items`: whole ordered `List(T)` batches |
| `mfn-core` | [`workflow.loop`](#structured-loop) | Top-level `loop`: required `max_iterations`, `variables`, and `body`; optional `until` | One required initial-value input per variable | One required final-value output per variable |
| `mfn-code` | `builtin.code` | Required `language`, `inputs`, and `code` | Required ports named and typed by `inputs` | Required ports named by `code`, with inferred types |

The built-in Loop is registered as `workflow.loop`. Its settings belong in the node's top-level `loop`
field; omit `config` or leave it empty. See [Structured Loop](#structured-loop) for its body and control kinds.

`mfn-core` registers `builtin.iteration` for explicit dependency selection. The compiler and runtime orchestrate its
body because an ordinary node invocation does not schedule a subgraph. Body nodes can use other linked packages
declared by the enclosing workflow.

`builtin.code` requires an explicit `language` field; the supported value is
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

Build validation parses and type-checks every CEL output expression, including nodes on inactive branches, without
evaluating it. Output port types are inferred from the checked expression. Unknown names, incompatible operations,
explicit `dyn(...)` calls, and result types outside the JSON port contract fail before executable installation. CEL
compilation produces a checked AST for in-process evaluation; it does not generate native machine code.

At execution, Code converts each declared JSON input to its CEL type without coercion. `int` accepts signed 64-bit
JSON integers, `double` accepts finite JSON floating-point numbers, and `null` requires a present null value. Lists
and maps are recursive and homogeneous; map keys are strings. Each output expression uses the same input bindings,
and outputs cannot refer to one another. CEL evaluation errors and values that cannot be represented as the inferred
JSON type fail the node. Errors identify the Code input or output and a JSON Pointer path for nested values. The
workflow publishes no Code outputs if any expression fails. A skipped Code node does not evaluate expressions.

To convert text explicitly, declare a string input and call `int(text)` or `double(text)` in the expression.
The checker infers `Int64` or `Float64` outputs. Integers must fit signed 64-bit range; doubles can use decimal
or scientific notation and must produce a finite JSON number. Invalid numeric text fails the node.
For example, a readline output connected to input `text` can use:

```json
{
  "language": "cel",
  "inputs": { "text": "string" },
  "code": { "number": "int(text)" }
}
```

Each expression is limited to 8 KiB. The serialized input map and output map are each limited to 1 MiB, with at most
10,000 collection entries across input conversion and output conversion per execution. Type descriptors and values
are limited to 16 nesting levels. These bounds limit accidental work; CEL evaluation runs in-process without a hard
CPU or memory sandbox.

Run the complete [scalar example](../examples/cel-scalar.json) or [typed-list example](../examples/cel-list.json)
with the [repository development commands](compiling.md#repository-development). They produce `{"doubled":42}`
and `{"doubled":[2,4]}` respectively. Replace the examples' local package paths with published package versions
when compiling outside the checkout.

## Structured Loop

The [Loop example](../examples/loop.json) returns `{"count":3}`. It declares `mfn-core` for both its
initial constant and the `workflow.loop` declaration, and `mfn-code` for the body transformation.
The compiler replaces the Loop declaration with an engine-prepared executor. `workflow.loop_assign`,
`workflow.exit_loop`, and the synthetic `%loop` source remain engine controls that plugins cannot
register. The outer graph and every Loop body remain acyclic. The engine repeats a body's fixed
execution order instead of adding a graph back edge.

A Loop node has a typed `loop` field with required `max_iterations`, a nonempty `variables` list, an
optional `until` condition, and a `body` containing ordinary `nodes`, `edges`, and `control_edges`.
Each variable creates a required Loop input for its initial value and a required Loop output for its
final value. The body reads current values and a zero-based `index` through the synthetic `%loop`
node. Every body node ID is local to that body; `%loop` and `index` are reserved there. Cross-scope
edges and implicit reads of outer outputs are rejected. Import an outer value through a Loop input,
including when initializing a nested Loop.

```json
{
  "id": "repeat",
  "kind": "workflow.loop",
  "loop": {
    "max_iterations": 5,
    "variables": [{"name": "count", "type": "int"}],
    "until": {"variable": "count", "operator": "gte", "value": 3},
    "body": {
      "nodes": [
        {"id": "increment", "kind": "builtin.code", "config": {
          "language": "cel", "inputs": {"count": "int"}, "code": {"next": "count + 1"}
        }},
        {"id": "assign", "kind": "workflow.loop_assign", "config": {"variable": "count"}}
      ],
      "edges": [
        {"from_node": "%loop", "from_output": "count", "to_node": "increment", "to_input": "count"},
        {"from_node": "increment", "from_output": "next", "to_node": "assign", "to_input": "value"}
      ]
    }
  }
}
```

The surrounding graph binds the initial `count` input and may bind the final `count` output.
Variable types use the shared port descriptor grammar: `any`, `null`, `bool`, `number`, `int`, `uint`, `usize`, `float`, `double`, `string`, `array`, `object`, and nested `{"list": T}`, `{"map": T}`, or `{"nullable": T}`.
`builtin.code` continues to accept only its concrete subset. Initial values, assignments, and final outputs are checked against each variable's declared type; a broad source is checked at runtime when necessary.

`workflow.loop_assign` takes one required `value` input and produces a `done` control output. A
reached assignment overwrites its target variable; a skipped assignment leaves it unchanged. A later
body step that must read the new value needs an explicit data or control dependency on the
assignment. `workflow.exit_loop` has no data ports; when activated by a control dependency, it ends
the nearest Loop immediately and leaves the rest of that pass unvisited. A skipped exit has no
effect. Assignment and exit steps are invalid outside a Loop.

An active Loop runs at least one pass. Each pass starts with fresh body outputs and skip markers
while variable values persist. After a complete pass, `until` compares a declared scalar variable
with its literal using `eq`, `ne`, `gt`, `gte`, `lt`, or `lte`. Numeric comparisons retain the
existing exact JSON number behavior. A true condition stops the Loop; reaching `max_iterations` also
stops it successfully. If both happen on the same pass, the stop reason is `condition`. Without
`until`, the Loop runs to its maximum unless an exit step runs. A skipped incoming dependency skips
the entire Loop, including all its output ports, without running a pass. Body failure or budget
exhaustion fails the workflow without publishing partial Loop outputs; completed plugin side effects
are not rolled back.

`max_iterations` must be between 1 and 1000. Loops can nest four levels deep. A workflow run is
limited to 10,000 scheduled steps across all scopes, including skipped and structural steps.
Exceeding that budget fails before the over-budget step runs. Loop errors identify the scope and
pass index when execution has begun. This feature follows Dify's
[Loop](https://docs.dify.ai/en/cloud/use-dify/nodes/loop) pattern of sequential refinement; Dify's
array [Iteration](https://docs.dify.ai/en/cloud/use-dify/nodes/iteration), export format,
conversation variables, parallel execution, and error-continue modes are separate features.

## Validation

The CLI checks node IDs, edge endpoints, selected output names, reserved Loop controls, and cycles in every scope before generating runner code. The compiled runner requires a registered Loop declaration and validates ordinary kinds, configuration, ports, type compatibility, required input connections, and Loop state contracts before installation. Every failure returns a nonzero status and preserves an existing output executable.

Port connections are statically safe when the source type fits the target, such as `Int64` to `Number` or `List(Int64)` to
`Array`. A constant's configured value determines its output type, and `builtin.identity` carries known type and value
information from its data input. For example, a constant with value `42` remains `Int64` through two identity nodes.
Connecting that result to a `String` input fails compilation with both edge endpoints. A mixed constant `[1, "x"]` has
the broad type `Array`, but its known value still fails compilation when connected to `List(Int64)`; the diagnostic
identifies `/1`. Empty arrays can feed a typed list because their known value satisfies its element contract.

An unknown broad source can feed a refined target when the runtime checks the actual JSON value before invoking that
target: a plugin-declared `Any` output to `Int64`, `Number` to `Float64`, and `Array` to `List(Int64)` are examples.
Concrete conflicts such as `String` to `Int64` or `List(String)` to `List(Int64)` also fail compilation. No values are
coerced. Produced outputs are checked against their resolved types before publication, including outputs without
consumers. A runtime mismatch reports the node, port, and nested JSON Pointer path where applicable.

See [compiling workflows](compiling.md) to build and run a definition, or [node development](node-development.md) to add node kinds.

## Iteration

The [iteration example](../examples/iteration.json) runs a body graph once per input array element and returns
`{"results":[2,5,8]}`. It follows the array mapping, zero-based index, execution modes, and error policies described
by the [Dify Iteration node](https://docs.dify.ai/en/cloud/use-dify/nodes/iteration). It also accepts maps represented
as JSON objects. Add a `builtin.iteration` node to the outer graph, connect an array or object to its required
`items` input, and read the collected array from its `results` output. An empty array or object returns an empty
array after the body has passed validation. Other input shapes fail at execution before any body invocation.

```json
{
  "id": "iteration",
  "kind": "builtin.iteration",
  "config": {
    "mode": "sequential",
    "on_error": "terminate",
    "body": {
      "nodes": [{ "id": "copy", "kind": "builtin.identity" }],
      "edges": [
        { "from_node": "%iteration", "from_output": "item", "to_node": "copy", "to_input": "input" }
      ],
      "result": { "node": "copy", "port": "value" }
    }
  }
}
```

The enclosing workflow must declare `mfn-core` for both `builtin.iteration` and `builtin.identity`. `%iteration` is a reserved body
source with outputs `item` (the current array element or map value), `key` (the map key, or JSON null for an array),
and `index` (a zero-based signed integer). Maps are visited in lexicographic key order; their `index` and collected
results use that order in both execution modes. The outer `items` input holds the complete collection.
For example, the body above maps `{"b":2,"a":1}` to `[1,2]`.
Connect `%iteration.key` to a body input to include the key in a transformation. Body
nodes, data edges, and optional `control_edges` use the ordinary workflow graph rules. The required `result` selects
one body port for each item. Body node IDs belong to the body scope; outer edges cannot address them. All body nodes
are constructed and validated before the runner is installed, including when the input collection is empty.

`mode` defaults to `sequential`. `parallel` schedules up to ten item jobs through the runtime worker pool and keeps
results in input order. Actual concurrency is also capped by `RuntimeOptions.max_parallel_domains` and, for stream
instances, `execution.limits.workers`. Parallel mode is suitable when body operations are independent. Nodes in the body may be invoked repeatedly and concurrently, so a plugin with
mutable internal state must synchronize it or use sequential mode. Each invocation gets fresh context values for
`%iteration.item`, `%iteration.key`, and `%iteration.index`; body outputs from another item are never visible.

`on_error` defaults to `terminate`. With `terminate`, the first failing item stops sequential execution and fails the
iteration without publishing a partial result. Parallel execution stops scheduling new items after a failure, lets
already started items finish, and reports the lowest failed input index among those started. `continue_on_error` places
JSON null at each failed input position. `remove_failed` omits failed results while retaining successful input order.
A skipped body result counts as an item failure. `continue_on_error` exposes `List(Any)` because the array may contain
null; the other policies expose a list of the selected body's output type.

The current scope supports one iteration level. Body graphs cannot contain another Iteration node, a structured Loop construct, or access outer
context outputs directly. Pass values through the input collection or add nodes inside the body. The runner description and
terminal UI show the Iteration node as one outer graph node. OTel emits separate item and body-node spans and logs with
the outer node ID and item index; failures remain visible even under `continue_on_error` and `remove_failed`. See
[observation contracts](observability.md#iteration-observation). Answer-node streaming is outside the current workflow
runner's output contract.

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

A selected output can fan out to multiple downstream nodes; all eligible consumers execute. Only one branch output is active per router invocation. A node gated by mutually exclusive outputs is skipped rather than acting as a merge. Compound boolean expressions and field-to-field comparisons are deferred.

Construction validates nodes, dependencies, message ownership, and execution-domain plans before returning an
executable Flow. `FlowRuntime` consumes prepared plans; it does not construct nodes or return graph construction
errors. Generated projects resolve linked providers in their Cargo build script and emit constant dependency,
execution-domain, and message-ownership tables. `prepare_workflow` binds fresh executors to those tables; launch
does not rebuild or repartition the graph. Loop and Iteration bodies use the same compiled layouts. Runtime
input validation, worker startup, and node execution can still fail after construction succeeds.

The runtime groups task nodes into synchronous execution domains. A linear chain stays in one domain and runs in
topological order. Forks split into separate branch domains, which can run concurrently when ready; a join waits
for every predecessor domain. Concurrency is bounded by `RuntimeOptions.max_parallel_domains`, which defaults to
four. Independent domains may complete side effects in either order, while dependencies and selected output order
remain stable. In stream workflows, execution domains are distinct from message domains, which continue to define
message identity and FIFO ordering. Domains inside Loop and Iteration scopes run serially in topological order to
preserve scope writes and exit behavior; Iteration can still process separate items in parallel through the same
bounded worker pool.

## Workflow startup parameters

Schema `2026-10-03` promotes the inputs of top-level initial nodes to workflow parameters. An initial node
has no incoming data or control edges. Its configured input types and required flags define the interface;
the workflow does not repeat those declarations. A node behind a control edge still needs its required
data connections. Loop and Iteration body inputs stay local to their enclosing scope.

Supply a JSON object keyed by exact node ID and then input port. Names containing dots remain whole keys:

```json
{
  "read": { "path": "/data/events.jsonl" },
  "copy": { "input": { "count": 42 } }
}
```

All startup arguments are checked before any node executes. Unknown nodes/ports, missing required values,
duplicate JSON keys, and type mismatches fail with input context. Optional omissions stay absent; explicit
null is checked against the port type. Parameter values do not specialize the compiled graph or persist
between invocations. `describe_workflow_inputs` inspects configured requirements without executing nodes;
`execute_compiled_with_inputs` and `Flow::execute_with_inputs` bind values through the same validator.

Older single-run schemas keep their required-edge rules.

## Streaming through the in-memory API

Schema `2026-10-03` enables streams with `execution: {"mode": "stream"}` and optional limits. Initial
nodes receive their workflow parameters once. Initial tasks and their ordinary dependencies execute in one
startup frame; producers execute independently and emit messages into their own domains. An empty graph or a
task-only graph finishes without external input. A producer reached through startup tasks also starts once.

Use `builtin.readline` to read UTF-8 text lines from a file or stdin. Its optional string input `path`
selects a regular text file when supplied, including through a symbolic link to a regular file. FIFO paths,
directories, devices, and sockets are unsupported and cause execution to fail. A FIFO path is rejected
without waiting for a writer. Omission selects stdin, which can be a pipe or terminal.
Its string output `line` preserves blank lines and whitespace and strips LF/CRLF delimiters. JSON parsing
belongs in downstream nodes. Other StreamNodes can
obtain data from files or services using their startup parameters. Every source is an ordinary declared node;
there is no injected node or global input type. Initial event nodes require an upstream activation source.

Task branches can rejoin within one message domain. Event and producer emissions create new domains. Joining
an earlier item with a collected output, broadcasting startup values into emitted frames, or joining
independent sources requires an explicit correlation operation, which is currently unsupported. Context
references and selected outputs respect the same boundary. Synchronous bodies still contain only tasks.

Producers close by returning. Each source drains
independently; workflow completion waits for every source, downstream work, and selected output delivery.
See [the instance API](node-development.md#in-memory-streaming-instances) and
[runner instructions](compiling.md#streaming-runners).

### Batch collection

Link `mfn-core` and insert `builtin.batch` into a streaming graph. Its required configuration fields
are positive `max_items` and `max_wait_ms`. Connect an upstream value to `item`; `items` emits the entire
ordered array, with inferred type `List(T)` for input type `T`.

The first item starts the timeout. Later items do not extend it. Reaching the count threshold emits a
full batch; timeout or upstream close emits a nonempty partial batch. An item arriving at the deadline
belongs to a new batch. Empty buffers emit nothing. A skipped input adds no item, while existing
buffered items retain their deadline. Arrays and null remain individual elements.

Downstream nodes receive one invocation per emitted batch. Input and batch values belong to different
message domains. Failure discards a partial buffer without flushing or retrying.

Batch runs through the in-memory API and standalone runners, and is rejected inside synchronous
Loop/Iteration bodies. Its state is independent for every workflow instance.
The [streaming Batch example](../examples/stream-batch.json) demonstrates the standalone input and output contract.
