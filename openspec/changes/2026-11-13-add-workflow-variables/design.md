## State ownership

`variables` is a definition-level list of `{name, type, initial}` values, not a node. `initial` is
mandatory, including for nullable variables; an explicit null is not an omitted default. It is
available in each callable Flow, including independently invoked child Flows, and absent from
container bodies. Lowering injects the synthetic `%root` source with the canonical ID and kind, so
readers keep using ordinary `{node, port}` references. The source exposes typed output ports, not
inputs or an intrinsic index. It is not authored, cannot receive dependencies, and cannot own
children. Variable names are exact nonblank strings and must be unique.

Lowering freezes a `RootScopeRelations` record containing the source endpoint, variable descriptors,
initial literals, and assignment instruction targets. Readers of compiled graphs use these facts,
not provider names or configuration discovery. Root state and container state cannot coexist in the
same graph. A Loop body continues to use its own frame; a callable Flow inside a Loop gets fresh root
state and cannot mutate its caller's state.

## Implicit scope sources

Authors reference `%root`, `%loop`, and `%iteration` directly in input bindings and output selections;
they never declare the source nodes. `workflow.assign` also needs no source declaration. Before
injecting an internal source, lowering rejects authored nodes with any reserved source ID or internal
source kind, including a declaration that exactly matches the canonical internal node. Only the
current authorized scope is checked; ordinary configuration values remain opaque.

Source instructions remain in compiled graphs for type inference, scheduling, and publication.
Compiled-artifact replay consumes stored relationships and does not re-enter authoring lowering.
Resolved-definition adapters may retain canonical sources inserted by an earlier pass, but cannot
introduce a source outside its owning scope or with a forged ID, kind, or configuration. This internal
path is not selected by authored files. No input-reference grammar, assignment ordering, state
lifetime, or observation path changes are needed.

## Assignment and ordering

`workflow.assign` uses the existing `{variable}` configuration, required typed `value` input, and
`done` control output. It targets the current graph's writable state, never searches through caller
or container boundaries, and cannot name a read-only input. `workflow.assign` targets the nearest writable state: a Loop frame when one is present, otherwise root state.
`loop.exit` remains Loop-only; definition-level variables do not authorize an exit.

Writes are validated and staged before publishing `done`. Successful publication commits the write
and refreshes the `%root` output. Skips preserve the variable and skip `done`; failure
publishes neither the write nor completion. Native payload identity and secret metadata survive.
A reader requiring the new value must depend explicitly on `assign.done`, in addition to its data
binding from the `%root` source. A cached ordinary node output is not retroactively recomputed.

A source task reads the current variable map, so assignment before source visitation cannot reset a
value to its initial literal. Selecting a `%root` output at workflow completion returns its final
value. The source does not impose a hidden barrier or infer read-after-write dependencies.

## Execution lifecycle

Root state is stored separately from the `ExecutionScope` stack. It is installed for one finite Flow
execution and restored on success, error, and unwinding. It does not hide startup arguments or add a
Loop path. Calls use existing isolated contexts. Each invocation starts from the declared initial
literals, even when prepared executors or caller contexts are reused.

Root-state Flows run execution domains serially through existing fork/merge mechanics. Domain state
copies preserve committed writes and sensitivity-only changes. Their worker pool remains available
for node-owned parallel work, but competing graph domains cannot race writes. Typed edge transfer is
conservatively disabled for root-state graphs; generated runners use the shared dynamic publication
path. Unrelated ordinary Flows retain existing parallel and typed execution.

## Integration with Loop output updates (#293)

PR #293 adds end-of-pass Loop updates from body outputs; this change adds immediate assignment and
invocation-local root state. They are complementary. Keep batch update policy in the Loop provider;
do not reinterpret callable Flow outputs as root-variable updates. Generic Loop assignments already
lower into `ScopeRelations::writes`, so the provider's overlap check must cover both assignment kinds
without inspecting kind labels.

This change is stacked on #293 and reuses its existing `2026-11-13` version declaration, with separate
feature predicates for Loop output updates and workflow variables. Its exit-before-output-selection
check remains inside the root-state execution closure, so abandoned Loop outputs are never read.
Both examples are registered in the CLI example inventory. Combined regressions cover generic
immediate assignments overlapping batch targets, distinct targets, early exits, and root/Loop state
isolation in memory and source-free generated runners.

## Preparation and compatibility

Version `2026-11-13` gates the two new engine kinds. They cannot be registered by plugins. Missing
initial values, malformed descriptors, unknown targets, invalid nested ownership, and root-state
streaming definitions fail before execution. Factory-selected top-level streaming is also rejected
after execution-mode resolution. Finite TaskStream nodes retain ordinary terminal adaptation, and
finite callable bodies can forward emissions through their existing sink path.

The runtime validates stored root relationships when describing artifacts, and the compiler validates
them before preparing providers. Generated preparation binds the same schema to its frozen Flow;
executables do not read source files. Root state instructions are not isolated node targets because
their semantics require a complete owning invocation. Observation sees ordinary nodes in the
existing root or Call scope, not fabricated Loop pass events.
