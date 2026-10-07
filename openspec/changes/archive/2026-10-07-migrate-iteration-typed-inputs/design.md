# Design

## Context

`IterationNode` is public and implements `TaskNode`. Its required `items` port is fixed at Any, while its result descriptor depends on its prepared body and failure policy. Preserve this public boundary while using runtime-owned typed decoding.

## Decisions

- Export `IterationInputs` with `items: ValueRef` and derive `NodeInputs`; array/map validation remains provider business logic.
- Implement `TypedTaskNode` and use typed preparation in the registered factory. A private result-port helper shares output construction between the public complete `ports()` method and factory metadata.
- Keep `TaskNode` for existing consumers. Its implementation delegates to `execute_typed_task`, a runtime function also used by the private typed adapter. This avoids provider-written map decoding and preserves existing dynamic callers without another wrapper type.
- Retain the existing iteration tests and extend their assertions to cover metadata derivation, typed calls, shared handles, and input errors. Existing compiler tests cover scalar rejection under every mode and policy, nested bodies, and generated execution; avoid duplicating those scalar cases in provider tests.

## Risks / Trade-offs

- Callers importing both execution traits must qualify the execution method; document both entry points.
- Invalid direct dynamic inputs become shared decode errors for missing/unknown ports. Scalar items still fail before running the body under the existing provider diagnostic.
- The runtime helper performs input conversion and invocation, not scheduling or publication; document its equivalence to direct task invocation.
