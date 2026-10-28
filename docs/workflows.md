# Workflow definitions

## JSON format

Use version `2026-10-25` for new workflows. Each node has a unique nonblank `id`, a registered `kind`, optional
`config`, and optional `inputs`. Inputs reference producer outputs; `edges` declare execution prerequisites. An
`outputs` entry selects a node port and names the executable's JSON result. Version `2026-10-23` also supports
display-only [node clusters](#node-clusters).

The [conditional convergence example](../examples/exclusive-inputs.json) and the [basic
example](../examples/hello-workflow.json) use this format with annotated clusters. Existing supported versions keep their
original field meanings and feature restrictions; new fields are not inferred from their shape.

### Input references

Declare a source next to its consumer input:

```json
{
  "id": "print",
  "kind": "example.print",
  "inputs": {
    "text": {"from": {"node": "read", "port": "text"}}
  }
}
```

The kind above is illustrative. This reference binds `read.text` to `print.text` and establishes the scheduling
dependency. It needs no additional execution edge. Source node and port names are separate opaque strings; dots are not
parsed. References select whole output values, preserving sharing, types, and secret metadata. Use existing
transformation nodes to select fields or construct another value.

Input names must match the provider's ports. Duplicate keys, unknown fields, null bindings, and mixed binding variants
fail. A connected optional input still skips its consumer when its source skips. Unbound inputs keep their existing
required, optional, and startup parameter rules. Loop and Iteration bodies use the root version and the same binding
syntax within their existing scope and message-domain restrictions.

## Reusable Flows

Version `2026-10-25` adds `workflow.call`, a terminal node supplied by `mfn-core`. Declare reusable
sources in `flows`; Cargo node packages remain in `dependencies`:

```json
{
  "flows": {
    "normalize": { "path": "./flows/normalize.json" },
    "title": { "builtin": "mf-chat.chat-title" }
  },
  "nodes": [
    {
      "id": "normalize",
      "kind": "workflow.call",
      "config": { "flow": "normalize" },
      "inputs": {
        "input.text": { "from": { "node": "read", "port": "text" } }
      }
    }
  ]
}
```

The snippet omits the surrounding graph. Each source accepts exactly one `path` or `builtin`.
Paths resolve relative to their owning canonical definition file. Sources can call other sources;
recursive references and more than sixteen nested calls fail. The combined Call/Loop/Iteration
container depth is bounded at twenty. The compiler freezes resolved bodies in the executable; running,
inspecting, and isolating a compiled call do not require the original JSON files.

A call exposes the child startup interface using qualified port names such as `input.text`, and
exposes its selected outputs using `outputs.name`. These names form the reusable Flow's public
contract. Input and output types are checked during preparation. Explicit call inputs override child
environment bindings and defaults. A connected skipped producer skips the call, even when that input
has a default. Child optional outputs retain normal skipped-port semantics. Shared values and secret
markers cross the boundary without JSON conversion. Configuration references stay local to each graph.

Calls have isolated input bindings, outputs, and Loop state. They share cancellation, bounded workers,
future driving, and the caller's remaining step budget. Provider failures retain their typed error
chains with call context. Children cannot consume stdin or expose stream ports in this version;
terminal LLM calls are supported, and a parent may independently stream its answer. Calls inside an
Iteration body retain its task-only restriction and cannot hide another Loop or Iteration.

All referenced providers join the root dependency build and adjacent lock. Each child's provider paths
resolve against its own file. Compatible declarations of the same package combine features and default
features; different source declarations fail explicitly. Built-ins use matching caller-selected provider
packages when available and retain the Flow provider's declared requirements otherwise. In a development
checkout, declare the built-in's local providers explicitly, as in the nested title example.

Built-in names use `<package>.<flow>` to avoid collisions across providers, and lookup matches the
complete qualified name. `register_builtin_flow!` captures the provider's Cargo package automatically,
and the registry qualifies each local Flow name. The compiler resolves this generic
catalog without owning application definitions. `mf-chat` owns `workflows/title.json` and registers
`mf-chat.chat-title`; the CLI already links that provider. Custom hosts must link the provider before source
resolution. Standalone runners contain the expanded graph and need only its node packages.

The built-in `mf-chat.chat-title` Flow takes `input.prompt`, `input.text`, `input.status`, `input.context`,
`input.model`, `input.base_url`, and `input.api_key`. It checks the title policy in `context.title`,
captures provider failures, and returns optional `title` and required `usages`. An ineligible call
returns no title and an empty usage list; a failed provider attempt retains its usage without a title.
The [nested chat example](../examples/chat-nested-title-flow.json) combines the returned usage with the
conversation call and streams only the primary answer. The older flat title example remains available
for inspecting the equivalent composition.

Rust callers loading local references use `mf_compiler::load_workflow(path)?.definition` or
`resolve_flows(&definition, Some(directory))?.definition` before preparing the graph. Definitions with
only built-in references can also use the existing planning and compilation entry points directly.
`embed_workflows` resolves sources and emits Cargo rebuild dependencies for every referenced file.
For host-owned Cargo dependencies, use `load_embedded_workflow` followed by
`compile_resolved_definition`; provider declarations remain provenance and their paths need not exist.
`mf_chat::WorkflowProfile::from_path` provides this path-loading behavior for chat profiles.

## Node clusters

Version `2026-10-23` accepts an optional `clusters` array in the root graph and in Loop and Iteration bodies. Clusters
group existing nodes for inspection only; they do not add scheduling dependencies, input bindings, or executable
nodes. The [clustered example](../examples/clustered-workflow.json) compiles to the same result as
[the scalar example](../examples/cel-scalar.json).

```json
{
  "clusters": [
    {
      "label": "Load and validate",
      "annotation": "Read the input and enforce its schema invariants.",
      "nodes": ["read", "parse", "validate"]
    }
  ]
}
```

`label` is the short single-line title shown in the graph. `annotation` is the longer explanation shown inside the
cluster and in Details. Cluster geometry reserves the title's full terminal-cell width in both expanded and collapsed
views. Annotations wrap above the leftmost expanded member or below the collapsed node's summary, with space reserved so
connections cannot overwrite the text. Both views preserve line breaks and indentation. Details wraps annotations to
the pane width and scrolls by the visible page height.
Both are required and bounded. Members belong to the containing scope, must be unique, and a node can belong to at most
one cluster. Clusters are flat and cannot contain `%loop` or `%iteration` synthetic sources. A cluster may contain a
Loop or Iteration node as an opaque member.

In the terminal UI, `c` collapses or expands the cluster containing the selected node. A collapsed cluster is drawn as
one node with the short label and an aggregate status; connections to its members attach to that node and internal
connections are hidden. Expanding restores the member nodes while keeping the annotation visible. Cluster state is
local to the TUI session and never changes workflow results. Older TUI readers reject the new description version before execution.

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
| `mfn-core` | [`builtin.parameters`](#parameters) | Named parameters with types, optional `env` and `default`, and `secret` (default `true`) | One typed input per parameter; exposed at startup when initial | One matching typed output per parameter |
| `mfn-core` | `builtin.identity` | None | Required `input`: any value | `value`: the unchanged input, with its known type |
| `mfn-core` | `builtin.if_else` | Nonempty ordered `branches` | None; activated by execution prerequisites | One boolean activation output per branch, plus `else` |
| `mfn-core` | [`builtin.assert`](#assertions) | Required structured `condition` and static failure `message` | None; reads explicit upstream outputs from context | None; false fails execution |
| `mfn-core` | `builtin.iteration` | Body graph, mode, and item error policy | Required `items`: array or object | `results`: collected array |
| `mfn-core` | [`builtin.batch`](#batch-collection) | Positive `max_items` and `max_wait_ms`; streaming mode | Required `item`: `T` | `items`: whole ordered `List(T)` batches |
| `mfn-core` | [`workflow.loop`](#structured-loop) | Top-level `loop`: required `max_iterations`, `variables`, and `body`; optional `until` | One required initial-value input per variable | One required final-value output per variable |
| `mfn-code` | `builtin.code` | Required `language`, `inputs`, and `code` | Required ports named and typed by `inputs` | Required ports named by `code`, with inferred types |
| `mfn-llm` | [`builtin.llm`](#llm-configuration-and-conversation-values) | Model, endpoint, credential reference, generation parameters, deadline, and reply limit | Required `prompt`; optional `system` and `history` | Required `text` and updated `history`; optional `usage` and `finish_reason` |

The built-in Loop is registered as `workflow.loop`. Its settings belong in the node's top-level `loop`
field; omit `config` or leave it empty. See [Structured Loop](#structured-loop) for its body and control kinds.

`mfn-core` registers `builtin.iteration` for explicit dependency selection. The compiler and runtime orchestrate its
body because an ordinary node invocation does not schedule a subgraph. Body nodes can use other linked packages
declared by the enclosing workflow.

`builtin.code` requires an explicit `language` field; the supported value is
`cel`. Input names have explicit type declarations, while output names and expressions live in `code`. The CEL checker
infers each output port type from its expression:

```json
{
  "language": "cel",
  "inputs": {"amount": "int"},
  "code": {"doubled": "amount * 2"}
}
```

The common input and output port contract is language-independent. A future backend can interpret its `code` payload
without changing existing CEL definitions.
CEL input types include `int`, `uint`, `double`, `bool`, `string`, `null`, `any`, `object`, `array`, and nested
`{"list": T}` or `{"map": T}` descriptors. Maps have string keys; values must satisfy the declared element type.
`any` accepts any JSON value, `object` accepts JSON objects, and `array` accepts JSON arrays. Dynamic contents may be
heterogeneous. For example, `{"list":"object"}` supports records containing nested objects, lists, and scalars.
The checker reflects dynamic results as `Any`, `List(Any)`, or `Map(Any)` while retaining concrete types where known.
Concatenating two `List(Object)` inputs with `left + right` infers `List(Map(Any))`, compatible with `List(Object)`.
`number`, `nullable`, `usize`, and `float` remain unsupported Code input descriptors.

Build validation parses and type-checks every CEL output expression, including nodes on inactive branches, without
evaluating it. Output port types are inferred from the checked expression. Unknown names, incompatible operations,
and result types outside the JSON port contract fail before executable installation. Explicit `dyn(...)` is accepted;
dynamic operations and broad-to-concrete connections retain runtime checks. A dynamic result is not proof that its
contents satisfy a downstream record's field contract. CEL compilation produces a checked AST for in-process evaluation; it does not generate native machine code.

At execution, Code converts each declared JSON input to its CEL type using the runtime's lossless numeric rules.
`int` accepts signed 64-bit values, including exactly integral floating inputs; `double` accepts finite floating values
and integers exactly representable in `f64`. Fractions, range overflow, precision loss, and negative-zero-to-integer
conversion fail instead of silently rounding. `null` requires a present null value. Lists
and maps are recursive and follow their declared types; dynamic members may be heterogeneous, and map keys are strings.
Each output expression uses the same input bindings,
and outputs cannot refer to one another. CEL evaluation errors and values that cannot be represented as the inferred
JSON type fail the node. Errors identify the Code input or output and a JSON Pointer path for nested values. The
workflow publishes no Code outputs if any expression fails. A skipped Code node does not evaluate expressions.

Dynamic conversion preserves stored JSON numbers: signed-range integers use CEL Int, larger unsigned integers use
CEL UInt, and floating-point numbers retain their representation, including negative zero. Declared `uint` inputs
use the runtime's lossless unsigned conversion rules. Missing object fields stay absent; null remains explicit data.
Dynamic results are checked recursively and cannot contain bytes, timestamps, durations, non-finite numbers,
non-string map keys, or embedded evaluation errors.

Code explicitly marks every output secret when any supplied input contains a secret, including nested secrets.
This conservative policy also marks independent or constant outputs secret. Business JSON values remain unchanged;
snapshots and diagnostic value formatting use existing redaction. Public inputs alone produce public outputs.
Code does not preserve internal payload identity or provide fine-grained field sensitivity tracking through CEL.

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

### Assertions

`builtin.assert` uses the same structured `condition` as [IfElse](#if-else-if-and-else): `source`, `operator`,
and an optional comparison `value`. It succeeds when the condition matches and fails execution with its configured
static `message` otherwise. Invalid conditions fail at build time; unavailable comparison operands and evaluation
errors fail execution with source, path, and operator context. Diagnostics do not include source values. Keep secrets
out of the static message.

Assert has no input or output ports. Condition references must name an explicit upstream output and do not add execution
dependencies. In version `2026-10-19`, use node-success edges to order the producer, assertion, and subsequent Task:

```json
{
  "nodes": [
    {
      "id": "validate",
      "kind": "builtin.assert",
      "config": {
        "condition": {
          "source": {"output": "check.result", "path": "/valid"},
          "operator": "eq",
          "value": true
        },
        "message": "current identity is inconsistent"
      }
    }
  ],
  "edges": [
    {"from_node": "check", "to_node": "validate"},
    {"from_node": "validate", "to_node": "continue"}
  ]
}
```

This fragment assumes `check.result` contains a `valid` field and `continue` is another Task in the same scope.
All IfElse operators, JSON Pointer rules, numeric comparisons, and existence semantics apply to Assert. Use Code for
compound expressions, then assert its boolean result with `eq` and `value: true`.
An assertion on a skipped branch is itself skipped and does not activate its success dependents.

### Parameters

`builtin.parameters` declares typed inputs shared by runner arguments, environment variables,
and configured defaults. Environment and default sources require a top-level initial node,
without incoming data or execution dependencies; Loop and Iteration bodies cannot declare these sources.
Without environment or default sources, the node may consume upstream inputs and apply its
`secret` policy at that boundary. Only initial nodes expose their inputs as workflow startup parameters.

```json
{
  "id": "params",
  "kind": "builtin.parameters",
  "config": {
    "parameters": {
      "token": {"type": "string", "env": "API_TOKEN", "secret": true},
      "endpoint": {
        "type": "string",
        "env": "SERVICE_URL",
        "default": "https://example.com",
        "secret": false
      },
      "timeout": {"type": "uint", "default": 30, "secret": false}
    }
  }
}
```

Each parameter declares an input and a same-name output with the configured type.
Connect `params.token` or `params.timeout` directly to downstream inputs. The parameter
map and its names must be nonempty. Types use the ordinary port descriptors, including
`bool`, `int`, `uint`, `string`, `{"list": "int"}`, and `{"nullable": "string"}`.
Unknown configuration fields and defaults that do not match their declared type are rejected.

```sh
./workflow --params.endpoint https://override.example --params.timeout 60
./workflow --inputs '{"params.timeout":60}'
./workflow --inputs-file parameters.json
./workflow --node params --params.timeout 60
```

For each parameter, resolution uses an explicitly supplied argument first, then its named
environment variable, then its default. If none is available, execution fails before any
node runs. Explicit null and empty strings count as supplied values. A present but invalid
argument or environment value fails without falling back. Strings retain their text;
other environment values use the same JSON conversion as named CLI arguments, including
`null` for nullable strings. Environment names must be nonempty and contain neither `=`
nor NUL. Non-Unicode values fail, and the combined environment text read per invocation is
limited to 1 MiB. Unlisted variables are never read.

Runtime input binding resolves sources and validates all parameters before invoking nodes.
Runner parsing and manifest inspection do not read the environment; each execution reads
its own process environment. In-memory, finite, streaming, and isolated node execution use
this same binding path. Manifests with source or secret bindings use startup interface
version `2026-10-16`; interfaces without bindings retain version `2026-10-11`.

`secret` defaults to `true` for each parameter and applies to every source. Values are marked
before input snapshots, then retain their markers through the node's matching outputs.
Set `secret` to `false` for public parameters. Downstream access and business serialization
retain original values; observation shows `[REDACTED]`. Configured defaults are embedded in
the workflow and manifest, so credentials should come from arguments or environment variables.
See [secret runtime values](observability.md#secret-runtime-values) for propagation rules.
Nodes that support [configuration templates](#configuration-templates) can consume these outputs directly.

### Configuration templates

Providers explicitly enable templates for execution-time configuration fields. LLM configuration supports them;
structural settings such as parameter declarations, node ports, and Loop bodies remain static.

| Configuration value | Meaning |
| --- | --- |
| `{"$ref": "params.model"}` | Read an output, preserving its JSON type and secret metadata |
| `{"$ref": "params.settings", "path": "/models/0"}` | Select a subtree using JSON Pointer |
| `{"$template": "model-{{ params.name }}"}` | Interpolate string outputs into a string |
| `{"$template": "{{ params.settings#/models/0 }}"}` | Interpolate a selected subtree |
| `"{{ params.name }}"` | Ordinary literal string; it is not evaluated |

References name complete output IDs. The producer must be an explicit ancestor through data bindings or execution
dependencies;
a template does not add an implicit dependency. References remain within the existing scope and message-domain
boundaries. Compilation rejects unknown outputs and missing dependencies. Execution rejects missing or skipped
values, missing paths, and incompatible field types. Interpolation accepts strings without implicit conversion;
use a whole-value reference for numbers, booleans, arrays, or objects. JSON Pointer uses `~0` for `~` and `~1` for `/`.

Templates are parsed during preparation and resolved on every invocation. Neither compilation nor manifest
inspection reads parameter sources. Whole-value references retain the selected value's sensitivity; interpolated
strings become secret if any selected value is secret. Use `secret: true` on credential parameters so their inputs
and outputs are also redacted. Literal configuration and parameter defaults remain embedded in workflow artifacts.

### LLM configuration and conversation values

The opt-in `mfn-llm` package registers `builtin.llm`, a combined node producing one authoritative terminal result and optional text deltas:

| Direction | Port | Type | Required |
| --- | --- | --- | --- |
| Input | `prompt` | `string` | Yes |
| Input | `system` | `string` | No |
| Input | `history` | `List(Object)` | No |
| Output | `text` | `string` | Yes |
| Output | `history` | `List(Object)` | Yes |
| Output | `usage` | `Map(Int64)` | No |
| Output | `finish_reason` | `string` | No |
| Stream output | `delta` | `Object` | Yes, per emission |

The `delta` payload contains `text` and `model_call_id`. A connected graph edge or selected native output
selects streaming requests for every model round; an unconsumed invocation sends ordinary requests and emits nothing.
There is no per-node streaming flag or fallback request. Terminal text, history, finish reason, and usages
come from the completed provider response. Delivered prefixes remain provisional on failure or cancellation.

The [answer-and-title example](../examples/chat-title-flow.json) declares only the answer as public:

```json
"outputs": [{"name": "answer", "node": "model", "port": "delta"}]
```

Selecting the answer delta infers streaming execution and ordinary JSONL output. The title call stays terminal-only,
and final `usages` includes both calls when title generation runs. A finite workflow selecting only terminal
ports keeps ordinary JSON execution. Distinct actual model calls carry their own `model_call_id`.
Generated text and SSE framing have no client-side byte limits.
The request deadline bounds provider setup and reads. Emission waits follow native backpressure and stop
when the consumer fails or the host cancels.
Sparse and absent usage stay sparse or absent; omitted counters are never synthesized as zero.

Each history object has exactly two required string fields, `role` and `content`. Conversation roles are `user`
and `assistant`, with nonempty text for ordinary messages. Tool exchanges additionally use `tool` records,
as described below. The current `prompt` is also nonempty. System instructions use the separate
`system` input. Missing or empty history starts a new conversation; a successful turn appends its user prompt and
assistant text to the supplied history. The workflow owns transcript persistence and trimming.

Static configuration has this shape:

```json
{
  "model": "example-model",
  "base_url": "https://api.openai.com/v1",
  "api_key_env": "MODEL_API_KEY",
  "temperature": 0.2,
  "max_tokens": 256,
  "timeout_ms": 60000
}
```

`model` is required and nonblank. Defaults are the OpenAI API prefix, `OPENAI_API_KEY`, and a 60000 millisecond deadline.
Generated text has no client-side byte limit. Optional `temperature` must be finite and between 0 and 2; optional `max_tokens`
must be positive. Supplied numeric fields cannot be null. Deadlines must be positive integers.
`base_url` is an HTTP or HTTPS API prefix without embedded credentials, query, or fragment. Configuration rejects
unknown fields, including `stream` and provider-specific request overrides. Literal credentials are rejected.

`model`, `base_url`, `temperature`, `max_tokens`, and `timeout_ms` also accept configuration
templates. Use `api_key` with a runtime reference to supply credentials from a parameter; it cannot be combined
with `api_key_env`. The latter remains a literal environment-variable name for existing workflows.

```json
{
  "model": {"$ref": "params.model"},
  "base_url": {"$ref": "params.base_url"},
  "api_key": {"$ref": "params.api_key"},
  "temperature": {"$ref": "params.temperature"},
  "max_tokens": {"$ref": "params.max_tokens"}
}
```

The [parameter-driven example](../examples/llm-parameters.json) declares these parameters and connects
`params.prompt` to `chat.prompt`, establishing the dependency for all configuration references to `params`.

```sh
mf compile examples/llm-parameters.json --output ./configured-chat
./configured-chat --params.model your-model --params.prompt Hello
```

Set `MODEL_API_KEY` in the execution environment. Override the endpoint with `--params.base_url` or other public
settings through their named arguments. Argument values take precedence over parameter environment fallbacks.
The same prepared node can execute with different credentials, models, endpoints, and limits on subsequent runs.

`mfn_llm::validate_config` checks literal settings and template syntax without looking up credentials, creating clients or runtimes,
or contacting the endpoint. Value structs use the runtime's nested object codecs, so malformed history has indexed
diagnostics. Optional output metadata is omitted when absent rather than encoded as JSON null. The package's optional
`tokio` feature forwards the workflow-managed backend; its native HTTP transport still depends on Tokio with that
feature disabled.

Preparation and manifest inspection do not read the credential environment variable or contact the provider.
Execution resolves templates or reads the credential variable for each invocation, so credential rotation does not require recompiling a workflow.
Requests use Chat Completions at the configured prefix's `/chat/completions` route; `OPENAI_BASE_URL` does not override
that configured endpoint. The node reuses a configured HTTP client, disables redirects and automatic retries, and
disables idle connection pooling so prepared nodes can outlive earlier workflow-managed reactors.
Each invocation enforces its resolved deadline through the runtime future driver; the shared client retains no
invocation-specific credentials, endpoint, or deadline.

Request messages are optional system instructions, the supplied history in order, and the current user prompt.
On success, returned history preserves the supplied records and appends the current user prompt and complete assistant
text, including intervening tool calls and results. System instructions are excluded and must be supplied again
for each turn. The node does not retain conversation state or automatically trim history. Unsupported history roles and response content
fail rather than being silently rewritten. A nonempty reply truncated at its token limit retains the `length` reason.

`usage` contains available `input_tokens`, `output_tokens`, and `total_tokens` counters; unreported counts do not become
zero. Compatible services reporting partial wire usage are adapted without changing Rig's message protocol. Unknown
finish reasons retain their provider value. Select optional metadata as optional workflow outputs when a provider may
omit it. Credential and provider errors retain typed sources while diagnostic display omits credential values and
arbitrary remote bodies.

The deadline covers setup and waiting for a terminal reply, with deadline and cancellation checks before returning
converted data. Cancellation or failure returns no outputs or updated history. Synchronous setup and conversion cannot
be forcibly preempted. Workflow concurrency limits bound invocations; waiting on the model still occupies a synchronous
workflow thread. Async hosts use a blocking thread for synchronous workflow entry points.

#### Nodes as LLM tools

The [tool example](../examples/llm-tools.json) exposes a callable CEL node. Its `amount` argument comes from
`chat`, while an ordinary connection supplies `factor` from a constant. The model sees only `amount`.
The same binding and execution rules apply to terminal tasks, TaskStream nodes, and callable Flow wrappers.

Declare `config.call_outputs` as a static name-to-type map on the LLM node, then connect those slots with
`calls.<caller>.<alias>.ports`. These are intermediate call arguments, separate from terminal outputs and `delta`.
Explicit port mappings take precedence. `defaults: "remaining_inputs"` opts into the remaining unbound target
inputs; externally wired inputs are never included. The alias becomes the provider tool name and must contain
1 to 64 ASCII letters, digits, underscores, or hyphens. An optional binding `description` describes the tool.

Provider argument objects use flat names unless explicit output names collide with default-input names.
A collision produces separate `outputs` and `defaults` objects. Requiredness, nullability, closed structs,
enums, tagged variants, collections, and descriptions come from the resolved public contract. All constraints
on a mapped argument apply together. Opaque/custom or recursive codecs and lossless binary32 contracts are
rejected before the first provider request; give such tools a supported input wrapper. Unknown external
credential schemas do not affect the public tool schema. Runtime decoded validation remains authoritative,
including exact numeric representation checks that a provider may not enforce.

A complete assistant tool batch is validated before dispatch. Calls run in response order, with isolated
invocation state and ordinary external prerequisites. Streamed arguments are buffered and checked for
complete, duplicate-free JSON before any tool in that response executes. Tool IDs correlate each assistant
call with exactly one result. History preserves these records:

```json
[
  {"role":"assistant","content":"","tool_calls":[
    {"id":"call-1","name":"multiply","arguments":{"amount":21}}
  ]},
  {"role":"tool","content":"{\"result\":42}","tool_call_id":"call-1"}
]
```

Supply all results before another user or assistant message. Plain history records retain their existing
shape; metadata stays local. Full SDK assistant content is replayed between rounds of one invocation,
while the public persisted history contains visible text and structured tool exchanges. Provider-specific
reasoning state is not persisted across workflow invocations.

`max_tool_rounds` defaults to 8 (range 1–32) and `max_tool_calls` to 32 (range 1–128). A tool round is one
assistant batch, followed by another model request, so the maximum provider request count is rounds plus one.
Both limits accept configuration templates. One `timeout_ms` deadline covers all rounds and tool calls;
runtime cancellation, invocation depth, and shared execution budgets remain in force. Limit checks precede
batch execution. Already executed tools are not rolled back when a later round fails.

Tool arguments and results are each limited to 64 KiB of JSON, argument batches to 256 KiB, and tool
declarations to 256 KiB. Oversized results are replaced with a generic error rather than truncated JSON.
Marked secret result values are redacted before entering provider messages. Unknown names, invalid arguments,
skips, and domain failures return generic tool feedback; typed runtime control failures stop execution.
The adapter does not retry requests or tools automatically. A model may explicitly request a tool again;
side-effecting tools must enforce their own idempotency policy.

`text` concatenates visible assistant text across rounds, matching emitted deltas. `history` preserves each
round separately. `usages` contains one entry per actual model request with a distinct call ID; aggregate
`usage` includes only counters reported by every round. With `capture_errors`, a recoverable failure returns
`status: "failed"`, the original input history, and collected request facts. Delivery failures and runtime
control failures still propagate.

```sh
mf compile examples/llm-tools.json --output ./tool-chat
./tool-chat --chat.prompt 'Use multiply to double 21'
```

The [terminal example](../examples/llm-terminal.json) accepts startup prompt, system, and history inputs.
The [Loop example](../examples/llm-loop.json) passes updated history through a Loop variable for two turns.
Choose an available model in each example's configuration and set `MODEL_API_KEY` in the execution environment.
An OpenAI-compatible endpoint can be selected with `base_url`. Compilation does not need the key:

```bash
mf compile examples/llm-terminal.json --output ./chat
./chat --chat.prompt Hello --chat.system 'Answer briefly'
mf compile examples/llm-loop.json --output ./conversation
./conversation --repeat.prompt 'Continue the conversation' --repeat.history '[]' --repeat.text ''
```

Persist the returned history and pass it as `chat.history` with a new prompt to continue a terminal conversation.
The Loop example repeats the supplied prompt twice while preserving the earlier reply in the second request.

## Structured Loop

The [Loop example](../examples/loop.json) returns `{"count":3}`. It declares `mfn-core` for both its
initial constant and the `workflow.loop` declaration, and `mfn-code` for the body transformation.
The compiler replaces the Loop declaration with an engine-prepared executor. `workflow.loop_assign`,
`workflow.exit_loop`, and the synthetic `%loop` source remain engine controls that plugins cannot
register. The outer graph and every Loop body remain acyclic. The engine repeats a body's fixed
execution order instead of adding a graph back edge.

A Loop node has a typed `loop` field with required `max_iterations`, a nonempty `variables` list, an
optional `until` condition, and a `body` containing ordinary nodes and bindings in the root definition's format.
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
`builtin.code` supports the descriptors documented under [built-in nodes](#built-in-nodes), excluding `number`,
`nullable`, `usize`, and `float`. Initial values, assignments, and final outputs are checked against each variable's
declared type; a broad source is checked at runtime when necessary.

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

The enclosing workflow must declare `mfn-core` for both `builtin.iteration` and `builtin.identity`. `%iteration` is a
reserved body
source with outputs `item` (the current array element or map value), `key` (the map key, or JSON null for an array),
and `index` (a zero-based signed integer). Maps are visited in lexicographic key order; their `index` and collected
results use that order in both execution modes. The outer `items` input holds the complete collection.
For example, the body above maps `{"b":2,"a":1}` to `[1,2]`.
Connect `%iteration.key` to a body input to include the key in a transformation. Body
nodes, data bindings, and execution prerequisites use the root version's ordinary graph rules. The required `result` selects
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

In version `2026-10-19`, `edges` is optional and never binds a target input. It supports two forms:

```json
{
  "edges": [
    {"from_node": "write_file", "to_node": "notify"},
    {"from_node": "route", "from_output": "ci", "to_node": "ci_request"}
  ]
}
```

An edge without `from_output` requires successful source Task execution, including output validation and publication.
The Task may declare no outputs. An edge with `from_output` requires that output to be produced; false and null still
activate it. Omission selects node success; explicit null or an empty output name is invalid.

| Source outcome | Node-success edge | Output gate |
| --- | --- | --- |
| Successful Task with no outputs | Satisfied | No source port exists |
| Successful Task with the selected optional output skipped | Satisfied | Target skips |
| Source node skipped | Target skips | Target skips |
| Source still running | Target waits | Target waits |
| Failure or cancellation | Existing termination policy; no activation | Same |

All logical input bindings and execution prerequisites are conjunctive. Missing required gate outputs are errors, including
behind another skipped dependency. The combined graph must be acyclic; self edges, duplicate execution edges, and
unknown endpoints are rejected. Independent nodes retain existing concurrency.

Node-success edges currently connect ordinary Tasks in the same scope and input frame. This includes Tasks inside
supported bodies and Tasks processing the same Stream message. Event/Stream operators, Loop/Iteration containers,
synthetic sources, and cross-domain success edges are rejected. Operator termination and container completion are not
inferred from one message or one body step.

Context references such as `load_order.value` still require an explicit ancestor. Data bindings and execution edges
establish that ancestry; mentioning an output in a predicate or configuration template does not. Source success does not
guarantee a particular optional output exists: context reads retain their existing missing/skipped checks.

## Exclusive input bindings

Declare alternative outputs with an `exclusive` input binding:

```json
{
  "id": "consume",
  "kind": "builtin.identity",
  "inputs": {
    "input": {
      "exclusive": [
        {"node": "local", "port": "value"},
        {"node": "ci", "port": "value"}
      ]
    }
  }
}
```

At least two distinct sources are required. Every candidate participates in ordering and type checks. Compilation must
prove each candidate pair mutually exclusive, normally through different outputs of one `builtin.if_else`. Unguarded
producers, producers on one branch, and unrelated routers are rejected even if runtime values happen to select only one.
Success of a router does not prove any branch; success of a Task gated by a branch retains that necessary activation
fact. Convergence retains only facts common to every alternative.

Candidates must belong to the consumer's scope and input message domain. Resolution waits for all candidate outcomes in
the current frame: one produced value binds the input; all skipped candidates skip the consumer. False and null are
values. Unexpected absence remains an error, and failure or cancellation never selects a fallback. Runtime resolution
trusts the static proof without repeating mutual-exclusion checks.

Selection preserves shared values and secret metadata. Different input groups resolve independently; carry correlated
fields in one object when they must come from the same branch. A skipped connected optional group still skips its
consumer. Collectors retain their existing buffer and deadline behavior.

Compile the [exclusive-input example](../examples/exclusive-inputs.json) and select either branch:

```sh
mf compile examples/exclusive-inputs.json --output ./exclusive-inputs
mf run ./exclusive-inputs
mf run ./exclusive-inputs --params.ci true
```

These produce `{"result":{"mode":"local"}}` and `{"result":{"mode":"ci"}}`. Keep the branch-output gates when converging
on a shared consumer. Ordinary inputs remain jointly required.

## Migrating legacy definitions

In older versions, `edges` maps outputs to inputs using `from_node`, `from_output`, `to_node`, and `to_input`. Marking
every edge to one input with `exclusive: true` declares alternatives. Legacy `control_edges` contains output gates
without `to_input`. Updated readers continue to support these definitions without rewriting files.

To migrate a definition and its bodies:

1. Set the root version to `2026-10-19`.
2. Move each data edge into its consumer's `inputs`, using `from` or grouping its exclusive alternatives.
3. Move the old `control_edges` entries unchanged into `edges`, and remove `control_edges`, including empty arrays.
4. Add portless success edges only where successful Task completion is the intended prerequisite.

Never replace a branch-output gate with node success: a router can succeed while skipping that branch. The new version
rejects `control_edges`, `to_input` and `exclusive` on execution edges; older versions reject node-local inputs. Bodies
cannot override or mix versions. New definitions require updated support packages and readers; unsupported versions fail
explicitly.

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

Each domain forks an immutable execution-context snapshot and records its writes separately.
The compiler fixes which inherited output keys are visible, including the message source for
stream domains. Forking shares state without copying the visible output map; completed sibling
branches cannot become visible merely because they finish earlier. Successful domains commit
their declared outputs while scope writes retain the existing Loop ordering. Execution snapshots
are independent of optional observation snapshots and do not retain execution history.

## Workflow startup parameters

Schemas `2026-10-03` and `2026-10-19` promote the inputs of top-level initial nodes to workflow parameters. An initial node
has no incoming data or execution dependencies. Its configured input types and required flags define the interface;
the workflow does not repeat those declarations. A node behind a control edge still needs its required
data connections. Loop and Iteration body inputs stay local to their enclosing scope.

The startup interface uses flat qualified arguments: version `2026-10-11` for ordinary
inputs, or `2026-10-16` when [parameter bindings](#parameters) are present. Recompile runners with an older
interface before launching them through the updated CLI. Definition versions `2026-10-03` and `2026-10-19` share this startup-interface contract.

Pass parameters directly as `--node.port VALUE` or `--node.port=VALUE`:

```sh
./workflow --read.path /data/events.jsonl --copy.input '{"count":42}'
```

String ports accept plain text. Numeric, boolean, null, array, and object ports accept JSON values;
nullable string ports also accept `null`. Use `=` when a string starts with `--`. Repeated parameters,
unknown names, and missing values are rejected. Qualified names are matched as complete keys, including
dots within node IDs or port names. Ambiguous qualified names fail during workflow preparation.

The in-memory API, `--inputs`, and `--inputs-file` use a flat JSON object with the same qualified names.
Named parameters cannot be combined with either JSON transport option. Named transport and the canonical
JSON forwarded to the runner must each fit within 1 MiB:

```json
{
  "read.path": "/data/events.jsonl",
  "copy.input": { "count": 42 }
}
```

All startup arguments are checked before any node executes. Unknown parameters, missing required values,
duplicate JSON keys, and type mismatches fail with input context. Optional omissions stay absent; explicit
null is checked against the port type. Parameter values do not specialize the compiled graph or persist
between invocations. `describe_workflow_inputs` inspects configured requirements without executing nodes;
`execute_compiled_with_inputs` and `Flow::execute_with_inputs` bind values through the same validator.

Older single-run schemas keep their required-edge rules.

## Combined terminal and incremental results

A combined provider declares terminal and incremental ports separately and executes once. Use ordinary
port references for both categories:

```json
{
  "outputs": [
    {"name": "increments", "node": "group.count", "port": "delta"},
    {"name": "total", "node": "group.count", "port": "total"}
  ]
}
```

This fragment assumes a combined provider named `group.count`, such as the counting provider in
[node development](node-development.md#combined-task-and-stream-execution). Node IDs remain opaque;
dots are not path separators. Incremental graph edges and output selections select streaming execution
automatically during compilation. A combined node with only terminal consumers keeps finite execution.
Finite scopes and isolated Task targets retain terminal-only adaptation without an unread receiver.

A combined node's terminal ports stay in its input domain; its incremental ports create a separate ordered
domain. One consumer cannot implicitly correlate values from different domains. A workflow may select
outputs from multiple domains, but each JSONL record contains only the fields selected in its own domain.
Per-domain ordering and native delivery acknowledgment remain enforced. Successful workflow completion
waits for connected work to drain, and terminal values are never reconstructed from increments.

Use application event nodes when several delivery kinds must share one ordered protocol. The conversation
Flow routes preview events and releases final settlement inputs on upstream close, so the graph owns their
ordering.

## Streaming through the in-memory API

Schemas `2026-10-03`, `2026-10-19`, and `2026-10-23` infer streams from Event/Stream node contracts
and consumed or selected incremental ports. `execution` accepts optional `limits` without a mode field;
its presence does not force a terminal-only graph into streaming execution. Initial
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
