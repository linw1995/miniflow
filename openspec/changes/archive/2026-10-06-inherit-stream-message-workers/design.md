# Design

## Message frame initialization

Store the effective positive worker limit once in stream shared state. Both startup and emitted message frames use this limit, computed as the minimum of runtime and stream worker settings. Attach the coordinator-owned pool's existing non-owning handle before scheduling downstream work. Existing context forks already propagate both settings.

Pool capacity can be smaller than the effective limit when fewer execution domains contain synchronous work. Retain the effective limit independently rather than deriving it from the pool's actual worker count.

## Focused coverage

Extend the existing worker probe to distinguish configured limits from actual pool size. Use the controlled source fixture and the built-in Batch node to exercise direct producer messages and event emissions. Repeated parallel Iteration messages must retain the effective limit and reuse the same worker thread in a one-domain pool.

Existing oneshot tests retain Loop and sequential Iteration coverage. The new regression concentrates on parallel Iteration, where the missing handle creates replacement pools.

## Review and validation

Run isolated implementation and test ablations, storing reports only under ignored `target/`. Review ownership, pool lifetime, zero-worker streams, frame isolation, and cancellation propagation. Validate focused regressions, repository hooks, pinned Nix checks, and strict OpenSpec consistency before archival.
