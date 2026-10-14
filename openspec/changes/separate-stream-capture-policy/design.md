## Context

The runtime's current stream recorder owns no historical values, but it still owns
encoding budgets, capture counters, boundary records, and transport faults. Finite
snapshot retention does not justify assigning these policies to stream execution.

## Goals / Non-Goals

Keep runtime changes limited to notifications carrying current input/output values
and existing invocation/message identities. Keep capture policy in telemetry and
historical retention in consumers. Preserve current user-visible inspection.

Changing finite snapshot internals or redesigning the stream data protocol is out
of scope. Transport budgets remain observation policy, not business value limits.

## Decisions

Use a cloneable runtime observer with one callback accepting identity, direction,
and borrowed ports. Its observation owner preserves existing startup pairing checks.
It holds no capture counters, byte/depth budgets, fault state, or historical values.

Ordinary tasks use their existing invocation/publication boundaries. Producers and
event callbacks supply each successfully admitted emission and its real message ID.
Temporary immutable references only bridge scheduler lock release; callbacks never
run under that lock. Capture ordering reflects callback delivery; message identity
continues to describe business admission ordering.

Move the current recorder into the existing telemetry stream data module. A small
borrowed value view bridges runtime values and the dependency-independent encoder;
it contains no encoding policy. Reuse the existing bounded walk without first
cloning the full business value into another JSON tree. Runner owners finish the
recorder after stream execution, including error exits, and report its diagnostics.

## Risks / Trade-offs

Capture callbacks must be nonblocking and must not unwind. The telemetry recorder
isolates its own sink failures and panics. The runtime cannot promise arbitrary
user callbacks are safe or impose a telemetry budget on them.

An attached observer keeps dynamic execution enabled even after its telemetry sink
stops. Avoid adding a second state/status protocol just to optimize a fault path.

## Validation

Reuse existing live emission, partial publication, nested invocation, numeric,
omission, sink fault, generated runner, and receiver tests. Add a raw callback
assertion to an existing test proving runtime values are not subject to telemetry
budgets. Compare runtime source responsibilities and ablate that notification to
verify the regression assertion is meaningful. Keep reports under `target/`.
