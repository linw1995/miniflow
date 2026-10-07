# Node development

## Implement a node

Basic nodes share the `mfn-core` crate under `crates/builtin-nodes/core/`. Nodes with distinct dependencies belong in separate packages.

The optional [`mfn-code`](../crates/builtin-nodes/code/) package registers `builtin.code` with CEL expressions,
instance-specific typed inputs, and output types inferred by the CEL checker. A Flow must select the package
explicitly; `mfn-core` does not register this kind. See the [CEL examples](workflows.md#built-in-nodes).

A task provider depends on `mf-runtime`, implements `TaskNode::execute`, and registers a factory through
`inventory::submit!`. `NodeRegistration` declares a unique `kind` and its construction requirements.
The factory returns a `PreparedNode` containing the executor and its `NodeMetadata`. See
[constant](../crates/builtin-nodes/core/src/constant.rs) and
[identity](../crates/builtin-nodes/core/src/identity.rs) for working registrations.

Configured input declarations and stdin conditions must depend on the fixed configuration and selected provider,
not environment variables or process state. The generated build freezes them in the executable manifest, and
`--validate` and normal execution compare them with runtime preparation before dispatching nodes or reading sources.
Host and target implementations must expose the same configured interface even if executor initialization differs.
Initialization can still fail for an unavailable runtime resource; inspection returns the frozen interface without
initializing executors. Dynamic in-memory callers derive their interfaces normally and require no manifest.

`mfn-core` registers a subgraph factory for `workflow.loop`. The compiler prepares its body and passes
it to that factory, which constructs the complete Loop executor. A Flow using Loop must declare
`mfn-core` in its dependencies. `workflow.loop_assign`, `workflow.exit_loop`, and `%loop` remain reserved
engine kinds. Ordinary nodes inside a Loop body also require their packages in `dependencies`.

## Registration contract

A plugin can register several unique kinds from one crate. Dependency aliases do not change these names. Linking must explicitly retain each plugin crate, for example with `extern crate plugin_alias as _;`; a `kind()` function is not required by the runtime API. The [external multi-node fixture](../crates/mf-compiler/tests/fixtures/multi-nodes/src/lib.rs) demonstrates this contract.

All plugins and the consumer must resolve the same `mf-runtime` package identity, including its version and source. Registrations from a different runtime identity are not entries in the consumer's inventory.

Factories validate configuration and construct instances during build validation and execution. Keep external I/O
and business side effects in `TaskNode::execute` or `StreamNode::execute`; validation and `--describe` must not execute
the workflow. Description mode reads the embedded graph without calling factories, so factory diagnostics cannot
enter its JSON output. Building a Rust plugin can execute its build scripts and procedural macros with the build
user's permissions.

Use `NodeFactory::Plain` for a factory taking configuration and returning `PreparedNode`.
Use `NodeFactory::Subgraph` when construction also needs the occurrence ID, execution options, and a
`PreparedSubgraph`. The compiler supplies Loop and Iteration bodies through this second form. Missing
or inappropriate body arguments fail during preparation; an executor is returned only after binding.

## Select dependencies in a Flow

Declare plugin crates in the Flow's top-level `dependencies` object. The CLI generates imports for those packages and compiles a runner that validates and executes against the same registry. No predefined bundle or CLI rebuild is needed. See [workflow definitions](workflows.md) for registry, pinned Git, local-path, and feature syntax.

The CLI first checks graph structure, then builds the runner and invokes its `--validate` mode. Unknown kinds,
duplicate registrations, invalid configuration, and incompatible ports fail before installation. The runner's
normal mode executes generated node calls; validation never calls `TaskNode::execute` or `StreamNode::execute`.

The [multi-node fixture](../crates/mf-compiler/tests/fixtures/multi-nodes/) demonstrates one external crate registering several kinds. The packaged CLI acceptance script in [release prerequisites](releases.md) builds it from a packaged registry source outside the checkout.

See the [contribution guide](../CONTRIBUTING.md) for local checks and [dependency license auditing](licensing.md) before distributing additional plugins.

## Instance metadata

Factories supply input and output `NodePorts` in `NodeMetadata.ports`, using `PortSpec::owned` for
dynamic names. Names must be nonempty and unique within each direction. Metadata depends on
configuration and prepared-body output types, and is available before task execution.

The [Code node](../crates/builtin-nodes/code/src/lib.rs) uses this interface to expose ports from its declared inputs and checked CEL expressions.

Factories can populate `NodeMetadata.output_derivations` for fixed or forwarded outputs. Use
`OutputDerivation::literal("value", value)` for a configured JSON value, or
`OutputDerivation::forward_input("value", "input")` when the output always equals that input. An empty derivation list
retains declared port types. Derivations must depend only on configuration and
must describe the actual result whenever the output is produced. Validation rejects references to undeclared ports,
duplicate output derivations, and literal values that conflict with the output's declared type. A plugin that advertises
an inaccurate derivation can cause an incorrect compile-time decision; runtime port checks still reject values outside
resolved types.

`ValueType::infer_json(&value)` derives a bounded port type for a configured JSON value. It uses refined scalar,
homogeneous list, and homogeneous map types when possible, and broad `Number`, `Array`, or `Object` where a precise
descriptor is unavailable. Known JSON values remain available as separate evidence, so a mixed array can still cause a
compile-time mismatch on a typed target. The [constant](../crates/builtin-nodes/core/src/constant.rs) and
[identity](../crates/builtin-nodes/core/src/identity.rs) nodes demonstrate both derivation forms.

Port types include the broad JSON categories `Any`, `Null`, `Boolean`, `Number`, `String`, `Array`, and `Object`, plus `Int64`, `Float64`, and recursive `List(T)` and `Map(T)`. `Map(T)` describes an object with string keys and values of type `T`. Use `PortSpec::new` for static names and `PortSpec::owned` for generated names when constructing metadata:

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

Declare context reads in `NodeMetadata.context_references`. Each `ContextReference` contains a qualified output ID and a diagnostic label, such as a branch ID. Validation resolves exact `${node_id}.${output_name}` keys and requires the producer to be a strict ancestor through explicit dependencies. References do not add edges. Ambiguous qualified IDs are rejected with both source pairs.

## Context-aware execution

### Struct-defined inputs

Use the runtime's `NodeInputs` derive to declare input ports and decode their values from one owned struct:

```rust
use mf_runtime::{Inputs, NodeInputs, ValueRef};
use std::collections::BTreeMap;

#[derive(NodeInputs)]
struct RequestInputs {
    url: String,
    headers: Option<BTreeMap<String, String>>,
    body: Option<ValueRef>,
    #[input(rename = "request.path")]
    path: Option<String>,
}

let ports = RequestInputs::ports();
let request = RequestInputs::from_inputs(Inputs::from([
    ("url".into(), "https://example.test".into()),
]))?;
```

The derive delegates field decoding and diagnostics to runtime helpers. Supported owned fields are
`bool`, `i64`, `f64`, `String`, `ValueRef`, recursive `Vec<T>` and `BTreeMap<String, T>`, and top-level
`Option<T>`. Scalars retain strict JSON representations; integers do not become floating values.
`Option<T>` permits omission but retains T's port descriptor: omitted `Option<String>` becomes `None`,
explicit null is rejected, and supplied null for `Option<ValueRef>` becomes `Some` containing null.
Collection elements cannot be optional, and nested `Option` fields are unsupported.

Named-field structs support generics and type aliases. Tuple/unit structs, enums, and borrowed fields are
unsupported. Field names define port names; raw identifiers omit their `r#` prefix. Use
`#[input(rename = "port-name")]` for exact names, including punctuation. Empty and duplicate names fail
compilation. Input attributes are independent of Serde attributes and do not implement defaults or flattening.

The derive is re-exported by `mf-runtime`; providers do not need a separate macro dependency. A renamed
runtime dependency requires `#[input(runtime = "::runtime_alias")]` on the struct. Use
`#[input(runtime = "crate")]` when deriving within the runtime crate itself.

`InputDecoder`, `InputField`, and `InputValue` support manual input contracts. Their declarations and
decoders must agree on accepted names, requiredness, and value types. Errors retain typed mismatches;
`InputDecodeError::pointer()` adds the escaped port name to its nested path. Shared `ValueRef` payloads
remain shared during decoding, including collection descendants; owned strings and typed containers may allocate.

Implement `TaskNode::execute(inputs, &mut ExecutionContext)` and return `NodeResult`. Tasks can read
declared outputs through `ctx.output("source.value")`. `ContextValue` distinguishes a produced JSON value from
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

The runtime calls the task's single execution method. Loop assignment and exit use engine-owned scope
operations; publication remains validated by the runtime. A Loop body has its own output scope for
every pass. Context references inside that body resolve within the scope, including `%loop` outputs,
and still require an explicit ancestor dependency.

Keep output names local in node results. Runtime publication qualifies them with the instance ID. Explicit skipped names must be declared non-required outputs and cannot also be produced. A scheduler-skipped node propagates skipping through every output, including required ones. Context references alone never activate or skip a node.

Qualified output IDs must be unique. Both compiler validation and direct `mf_compiler::build_flow` construction reject collisions such as node `a.b` / output `c` and node `a` / output `b.c`, including optional outputs.

The shared executor checks every produced JSON value against its declared output type before publishing the node's result,
even when no downstream edge reads that port. It checks each bound input against its declared type before invoking an
active node. Nested list and map errors include a JSON Pointer path. A skipped node is not type-checked, and an
unexpectedly missing dependency remains an error before skip or type checks. Use `ValueType::Any` when a port legitimately
carries multiple JSON types; a specific declaration must match every produced value.

## Prepared execution nodes

`PreparedNode::new(task, metadata)` accepts `NodeMetadata` or plain `NodePorts` when no derivations or
references are needed. `FlowNode::new(id, prepared)` binds the definition identity. Compiler preparation
resolves the metadata before execution. Both in-memory execution and generated runners build a prepared `Flow`
and run it through `FlowRuntime`. The compiler partitions a DAG into synchronous domains before execution: tasks in one
domain run serially, while independent ready domains can run concurrently up to
`RuntimeOptions.max_parallel_domains` (four by default). Fan-in waits for all predecessor domains. Results and
context effects become visible to dependent domains only after validation and commit. Within Loop and Iteration
scopes, domain dispatch stays serial in topological order so scope writes and exits retain their defined order;
Iteration still parallelizes separate items through that same bounded worker pool. Workers waiting for nested item work
help execute queued pool jobs on their existing thread, so nested Iteration work shares the configured worker bound.

## Migrate an existing plugin

- Replace `impl Node` with `impl TaskNode`. Keep one `execute` implementation taking inputs and a
  mutable context. Convert ordinary output maps with `.into()` to return `NodeResult`.
- Move port declarations, output derivations, and context references into the factory's `NodeMetadata`.
- Return `PreparedNode` from factories and register them through `NodeFactory::Plain` or
  `NodeFactory::Subgraph`. Static input/output fields on `NodeRegistration` have moved into metadata.
- Construct container nodes with their prepared body. Remove declaration objects and `with_subgraph`
  methods that previously replaced them after construction.
- Rebuild the plugin and consumers against the same runtime package identity.

## Workflow observation context

Observed execution activates the workflow's OTel context and a node span around dependency resolution, invocation, and output publication. A node using an application-provided OTel tracer can create child spans through the current context without changing its execution interface. The runtime does not install a global provider or configure a plugin's tracer. Threads created by plugins require explicit context propagation.

Lifecycle events are emitted by the runtime independently of plugin diagnostic logs. Plugins continue to return values and explicit skipped ports normally; the runtime records success only after validating and publishing those results. See [observation contracts](observability.md) for provider ownership, failure phases, and a runnable SDK example.

## Event execution contract

`PreparedNode::new(task, metadata)` selects `NodeExecution::Task`.
`PreparedNode::event(state, metadata)` selects `NodeExecution::Event`. Event providers implement
`EventNode` directly; they do not implement `TaskNode` or create a second state object later.

Use `prepared.execution.as_task_node()` to borrow a task executor or
`prepared.execution.into_task_node()` to take ownership of it. Both return `None` for event and stream execution.
The consuming conversion moves only the execution field, leaving `prepared.metadata` available.

`EventNode::on_event` receives `Input`, `Timer`, or `UpstreamClosed` and returns zero or more complete
emissions with a `TimerUpdate`. `EventContext.now` is monotonic elapsed time.
Event state requires `Send`; mutable access is exclusive and
`Sync` is not required.

Oneshot preparation accepts task nodes and rejects event or stream nodes with their definition IDs. Direct callers
of the low-level task helper convert a prepared `FlowNode` with `into_task()` first. Normal `mf_compiler::build_flow` callers and
generated oneshot runners pass a task Flow to `FlowRuntime`; streaming preparation keeps the same Flow graph and
uses the runtime's stream lifecycle and message-domain scheduler.

## Startup input and resource declarations

Top-level initial nodes in schema `2026-10-03` expose their prepared input ports as workflow parameters.
Factories derive ports from configuration and do not need invocation values. Required flags and type
descriptors retain their ordinary port meaning. `WorkflowArguments::from_json` rejects duplicate keys and
limits JSON arguments to 1 MiB; `WorkflowInputSchema` validates the complete object before dispatch.

`NodeMetadata.stdin` optionally declares exclusive `StdinRequirement::Always`, or
`StdinRequirement::UnlessInput("path".into())` for a source whose optional input selects a file.
A data-bound conditional input removes its stdin requirement. Otherwise validated startup arguments determine
whether stdin is needed. Resource discovery uses metadata for built-in and external providers alike;
preparation does not acquire business inputs. Launchers use `WorkflowInputSchema::stdin_owner` and
`validate_stdin` to reject missing resources or competing active consumers before execution.

## Incremental stream producers

`PreparedNode::stream(producer, metadata)` selects `NodeExecution::Stream`. Implement `StreamNode`
when one input needs to produce many outputs incrementally, such as reading lines from a file.
The instance owns the producer, which requires `Send` and exclusive mutable access, without `Sync`.

```rust
impl StreamNode for Expand {
    fn execute(
        &mut self,
        inputs: Inputs,
        _context: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        for value in inputs["items"].as_array().expect("validated list input") {
            emitter.send(Outputs::from([("item".into(), value.clone())]).into())?;
        }
        Ok(())
    }
}
```

Each `Emitter::send` validates one `NodeResult` and waits until the node's pending queue has capacity.
Success transfers the result to the runtime; downstream processing can finish later. The queue limit
is `execution.limits.max_pending_messages`, defaulting to 64. A producer can emit more total results
than this limit. The emitter is borrowed for one invocation and cannot be cloned or retained afterward.
Factories declare ports and validate configuration; file access and other business operations belong
in `execute`. The [external line-producer fixture](../crates/mf-compiler/tests/fixtures/multi-nodes/src/line_producer.rs)
shows file reading with this contract.

The runtime lazily starts one dedicated worker for each producer that executes, then reuses that worker
and producer state across inputs. These workers are separate from the shared domain and Iteration worker pool
controlled by the lower of `execution.limits.workers` and `RuntimeOptions.max_parallel_domains`, so a blocked send
cannot consume the worker needed to drain its output.
Producer thread count is bounded by the graph's stream-node count. Timers remain coordinator-owned.

An invocation retains its input frame and may read declared ancestor outputs through its context.
Its emissions start a new message domain with FIFO order. Later inputs wait for that invocation to
return. Returning without sending emits no message. Closing workflow input waits for admitted
invocations, queued emissions, downstream close handling, and final sink acknowledgements.

Failure or dropping an unfinished instance wakes blocked sends with an error and prevents further
publication. Invalid emissions fail the instance even if the producer ignores a send error. Cleanup
waits for running producer calls and releases their state and workers. Arbitrary blocking plugin I/O
cannot be interrupted by the runtime; plugins should propagate send errors and return promptly.
Previously delivered outputs remain effective if a later read or operation fails.

Stream producers are supported in streaming workflows using schema `2026-10-03`, including generated
runners. They are rejected in synchronous flows, Loop bodies, and Iteration bodies. Existing task and
event interfaces retain their behavior; downstream exhaustive matches on `NodeExecution` must handle
the new `Stream` variant.

## In-memory streaming instances

`WorkflowDefinition::from_json` and `CompiledWorkflow::from_json` validate execution settings after
parsing. Direct Serde deserialization checks the document shape; compilation validates execution
settings for definitions constructed or deserialized by the host.

Schema `2026-10-03` accepts `execution: {"mode": "stream"}`. Initial task and stream nodes receive
workflow arguments once; ordinary edges retain their message identity, while producer/event outputs create
a new domain. Nodes and selected outputs cannot join different domains. Initial EventNodes require an
activation source, and nested synchronous bodies remain task-only.

Embedded node construction in `mf-compiler` returns `WorkflowBuildError`, preserving configuration and factory error sources.
Generated Cargo builds link the same provider packages and features to determine static executor boundaries.
Provider validation happens before the executable is installed. Launch initializes node state and attaches the
compiled graph tables without constructing a graph. Generated execution functions accept the prepared Flow
and return `WorkflowRunError`. `FlowRuntime` only executes prepared plans. Compiler orchestration helpers expose
preparation and execution failures through separate variants of `WorkflowExecutionError`.

Use `mf_compiler::instantiate_stream` to prepare independent node state and inspect its `plan()`. Pass
`WorkflowArguments` through `StreamOptions.arguments`, or use `start()` when no parameters are required.
Startup validates every argument and required resource before dispatch. Initial producers, including those
behind startup tasks, run on independent dedicated workers. Their contexts retain only values available at
dispatch. Ordinary per-message producer invocations remain serialized.

`builtin.readline` accepts optional string input `path` and emits string output `line`. Supplying a path
reads a regular UTF-8 text file, including through a symbolic link to a regular file. FIFO paths,
directories, devices, and sockets are unsupported. Paths are opened nonblocking and the opened file's
type is checked before reading, so rejecting a FIFO does not wait for a writer. Omission selects stdin,
which continues to accept pipes and terminals. It preserves blank lines and whitespace, strips LF or
CRLF delimiters, and accepts a final unterminated line. It does not parse JSON. Runners supply a reserved
`TextInput` when stdin is required; embedding hosts supply `StreamOptions.stdin` or use
`ExecutionContext::set_stdin` for context-based execution.
Factories and interface inspection do not open source files or read stdin. Runtime-owned reads check
cancellation between reads and while waiting for stdin readiness. Nonblocking opens prevent FIFO writer
waits; they do not make regular-file disk I/O interruptible. Application-specific sources use ordinary
`StreamNode` implementations and propagate emitter errors; arbitrary plugin I/O requires cooperation.

Every emitted message has fresh bindings and a step budget. Frames execute in FIFO order in each domain,
while domains progress independently. `receive` returns a delivery that its sink must acknowledge or fail.
Success waits for all sources and output acknowledgements. Failure stops admission and dispatch, wakes source
waits, suppresses later publication, and waits for started calls before releasing nodes. Dropping an unfinished
instance uses the same cleanup. The ordinary task worker pool remains separate from producer workers.

Limits default to 64 pending messages and four ordinary workers. Preparation reserves one startup frame and
one frame per output domain. Every operator's pending queue has finite message capacity. Pressure propagates back to producer emitters. Payload size and plugin buffers have no byte
quota in this layer.

A custom `StreamClock` must advance monotonically and wake registered instances. Deadline expiry
makes an emission ready; downstream execution remains subject to capacity. Snapshot capture is rejected
before startup. Streaming definitions also compile to standalone JSON Lines runners and support
stream-specific observation. See [streaming runners](compiling.md#streaming-runners).

Run the producer/consumer example with `cargo run -p mf-compiler --example stream` inside `nix develop`.

## Collection type derivation

Use `OutputDerivation::collect_input("items", "item")` when an output collects values from a named
input. Inference wraps its input type as `List(T)` and retains checks for broad element types. It adds
one level to the shared type-depth limit and discards exact-value evidence for the collected array.
The derivation must reference declared ports and fit the output declaration. Runtime publication still
validates every actual emitted value.

## Stream observation

Start a stream observation with `CompiledWorkflow::start_stream_observation` and pass it through
`StreamOptions.observation`. Invocations include run, message-domain, message, node, and nested-scope
identity. Timer and close callbacks have independent invocation identity even without an input frame.

Successful buffering reports zero emissions. Batch emissions carry item count and `size_exceed`,
`timeout_exceed`, or `upstream_closed` metadata. `EventNode::buffered_items` optionally reports a count
without exposing retained values. Transport and export queues remain bounded, and telemetry failures
do not change workflow results or request retries. Terminal events follow drain or failure cleanup.

The current terminal launcher and snapshot recorder remain unavailable for stream mode.

Workflow graph construction is owned by `mf-compiler`: `FlowBuilder::prepare` validates configured nodes and graph connections, then `into_tasks` or `into_stream` produces an executable runtime plan. `FlowBuildError`, `StreamBuildError`, and `WorkflowBuildError` are compiler errors. The runtime binds validated owned or borrowed plans and schedules executors without depending on the compiler. Node factories and their `NodeBuildError` contract remain shared provider contracts in `mf-runtime`.
