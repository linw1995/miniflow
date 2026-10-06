# Design

## Context

Execution contexts already propagate weak worker handles through domain and body forks. The missing boundary is the pool owner: oneshot execution creates its pool inside the concurrent domain scheduler, after its single-domain and serial fast paths. A scoped Flow also receives default runtime options even when its parent selected a different limit.

## Decisions

### Own the pool at the Flow entry point

A nonempty Flow execution without an active nonzero worker handle creates a pool at its execution entry point. The local owner remains alive until the invocation settles; all forked contexts continue to carry non-owning handles. Existing active pools, including stream domain pools, are reused.

Pool capacity follows the inherited runtime worker limit rather than the number of top-level execution domains. A single top-level domain can contain an Iteration with parallel items. Empty plans create no pool.

### Retain scoped ordering and inherited limits

Only an unscoped invocation applies explicit runtime options. A Loop or Iteration body retains the context's configured worker limit while scheduling its domains serially. This preserves Loop variable writes and exit cutoffs without reducing the shared pool's capacity for parallel item jobs.

The ordinary default execution entry point and existing cooperative wait implementation remain sufficient. No additional scheduling layer, compensation thread, concurrency counter, or public wrapper is introduced.

### Keep regression coverage focused

One probe records actual worker thread identities across repeated Loop passes or Iteration items. A one-worker case detects replacement pools, and a six-worker case checks that neither the default limit nor the single-domain plan shrinks the shared pool. Existing domain, scope, failure, panic, and generated-runner tests retain their existing responsibilities.

## Trade-offs

A nonempty oneshot Flow now starts its configured worker threads even when its own domains execute serially. This provides a simple run-wide ownership boundary for nested parallel work. Lazy pool ownership would require another shared lifetime abstraction and is deferred unless startup cost warrants it.

## Validation

Run focused worker and subgraph tests, existing Loop and Iteration tests, repository hooks, OpenSpec validation, and the full pinned Nix checks. Review implementation and specification consistency before archival.
