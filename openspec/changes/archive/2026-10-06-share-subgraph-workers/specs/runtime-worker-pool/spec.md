## ADDED Requirements

### Requirement: Share workflow workers with prepared subgraphs

A nonempty oneshot Flow invocation SHALL own one runtime worker pool until execution settles, unless its context has an active nonzero pool. New pool capacity SHALL follow the configured worker limit, independent of top-level domain count. Loop passes and Iteration items SHALL reuse this pool through non-owning handles. Empty Flows SHALL create no pool.

#### Scenario: Reuse workers across Loop passes

- **WHEN** a single-domain workflow repeats a Loop body containing parallel runtime jobs
- **THEN** every pass submits those jobs to the same workflow pool
- **AND** the workflow retains ownership until the invocation settles

#### Scenario: Run parallel Iteration within one top-level domain

- **WHEN** a single top-level execution domain invokes a parallel Iteration
- **THEN** item jobs use the workflow pool with its configured capacity
- **AND** the top-level domain count does not reduce that capacity

### Requirement: Retain inherited worker settings in scopes

Scoped Flow execution SHALL retain the context's configured worker limit rather than replacing it with default runtime options. Scoped domains SHALL retain their existing serial ordering, and parallel runtime jobs SHALL use the inherited workflow pool.

#### Scenario: Reuse workers across sequential Iteration items

- **WHEN** sequential Iteration bodies submit parallel runtime jobs under a nondefault worker limit
- **THEN** every item inherits that limit and the same workflow pool
- **AND** no item creates a replacement pool

#### Scenario: Reuse an existing pool in scoped execution

- **WHEN** a prepared Loop or Iteration body executes in a context carrying an active worker pool
- **THEN** the body retains that pool and inherited limit while its domains execute serially
- **AND** default body runtime options do not replace the inherited worker limit
