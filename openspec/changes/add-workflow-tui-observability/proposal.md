# Proposal

## Why

Compiled workflows currently expose final outputs and errors but cannot show which nodes are running, completed, or skipped. Users need a live terminal view while standalone runners expose reusable execution telemetry to OpenTelemetry consumers.

## What Changes

- Add a separate `mf-tui` crate used only by `mf-cli`; keep terminal UI dependencies out of generated runners and their support crates.
- Add `mf-telemetry` for versioned workflow observation contracts, execution instrumentation, and optional OTel SDK/OTLP export support.
- Emit workflow and node spans for execution analysis, plus independently exported OTel lifecycle events for live state updates.
- Add runner `--describe` output containing versioned graph metadata without node configuration or business values.
- Add `mf run <executable> --tui` to describe and launch an existing local runner, receive OTLP/HTTP on loopback, and display its graph, node states, durations, and diagnostics.
- Preserve execution semantics when telemetry is disabled, unavailable, or incomplete; distinguish conditional skips, nodes never reached after failure, and interrupted execution.
- Accept telemetry loss while exposing sequence gaps, local drops, and missing completion boundaries; define lightweight terminal records, bounded export, and shutdown behavior without state recovery.
- Scope each TUI session to one locally launched runner and one execution per node; omit attempt counters, retry scheduling, attach endpoints, and reconnect/replay machinery.

## Capabilities

### New Capabilities

- `workflow-observability`: Export correlated traces and lifecycle events with stable workflow execution semantics and explicit delivery limitations.
- `workflow-terminal-ui`: Observe a CLI-launched local runner and present its graph and live execution state through an isolated terminal UI.

### Modified Capabilities

- `workflow-binary-compilation`: Generate runners with graph description and telemetry initialization while preserving standalone execution and excluding TUI dependencies.

## Impact

- Workspace manifests and support-package distribution gain `mf-telemetry` and `mf-tui`, with OTel API/SDK, OTLP protocol, and terminal dependencies separated by responsibility.
- `mf-runtime` gains shared node instrumentation and workflow observation scope support; ordinary node execution interfaces retain their behavior.
- `mf-compiler` generates workflow observation boundaries, runner modes, and export lifecycle initialization.
- `mf-cli` gains child-process supervision, terminal ownership, and local receiver configuration.
- Runtime, generated-binary, protocol, dependency-boundary, and terminal integration tests cover the new contracts; workflow and compilation documentation describe the commands and telemetry fields.
- Retry scheduling, remote attach, and reconnection recovery are explicitly excluded requirements, with no follow-up work or extension hooks planned for them. Standalone export to an external Collector remains supported.
- Durable replay, workflow control, metrics dashboards, and simultaneous local/remote export are outside this change.
