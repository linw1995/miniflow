## Execution boundary

Loop stream exports map parent-facing aliases to local child TaskStream ports. They do not create
ordinary dataflow edges or mutate loop variables. The parent becomes a TaskStream only when it has
exports. Terminal-only adaptation retains the existing discard sink.

PreparedSubgraph carries one sink-aware execution callback. The compatibility constructor adapts its
ordinary callback to the same representation. TaskNode has a default sink entry point; TaskStreamExecutor
uses its existing emitter implementation. No borrowed sink is stored in ExecutionContext or sent to
worker threads. Connected child scopes execute synchronously through shared serial-domain machinery.

## Shared guards and publication

Finite success guards are stored by node identity rather than wrapped in separate domain executors.
The ordinary input resolver applies them for dynamic and generated invocation. Stream forwarding uses
the same resolver and node lifecycle, retaining snapshots, typed errors, assignments, and exit cutoffs.
Opaque custom semantic domain overrides are rejected for connected forwarding rather than bypassed.

Parent emitters enforce their own output contract after child validation. Different children produce
sparse packets; other aliases are skipped. Source requiredness is retained for a single child, while
multi-child exports are optional per packet. Backpressure and cancellation pass directly to the child.

## Generated execution and observation

Build-time layouts capture exact source PortSpecs and remap node indexes into generated order. Startup
checks the frozen contracts before enabling forwarding. No runtime graph reconstruction is introduced.
Native TaskStream completion carries the Loop summary to the existing observation lifecycle, keeping
one parent completion and per-pass child scopes.

## Scope and risks

Only finite TaskStream children are exported. General Event/Stream producers and workflow.call stream
exports remain outside this change. Slow consumers intentionally delay the active Loop pass. Delivered
prefixes remain provisional on failure, and arbitrary semantic domain overrides cannot be adapted
without an explicit streaming contract.
