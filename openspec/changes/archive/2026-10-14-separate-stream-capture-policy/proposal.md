## Why

Streaming inspection currently places payload encoding, byte/depth budgets, capture
sequences, loss accounting, and transport diagnostics in the runtime. These are
observation policies. The runtime already owns the current invocation values and
publication boundaries; it should expose those values without managing inspection
history or an export protocol.

## What Changes

- Replace the runtime stream recorder with a stateless observation callback.
- Move encoding and capture state to `mf-telemetry`, using borrowed runtime values.
- Let runner owners finish capture after execution; keep history in the TUI.
- Preserve admitted emission identities, out-of-lock callbacks, finite snapshots,
  and existing inspection behavior.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-observability`: separate runtime value notifications from capture policy.

## Impact

Runtime context/publication hooks, telemetry capture, generated runner wiring, and
their existing integration tests change. No dependency, wire version, or TUI storage
change is required. The new API introduced by this PR changes before release.
