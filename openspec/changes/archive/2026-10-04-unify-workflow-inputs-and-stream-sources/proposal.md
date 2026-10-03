# Proposal

## Why

Streaming workflows currently require an engine-owned `%input` source even when a node can obtain its own
data. This couples streaming to runner stdin and blocks TUI execution. Initial nodes should receive their
parameters through the workflow interface and start without an external trigger message.

## What Changes

- Derive workflow startup inputs from the configured input ports of top-level nodes with no incoming data or
  control edges. Apply the same contract to single-run and streaming workflows.
- Start initial tasks once and initial stream producers once per workflow instance. Preserve message-driven
  execution downstream of producers.
- **BREAKING**: Introduce definition schema `2026-10-03`, remove automatic `%input` injection and
  `execution.input_type`, and replace the instance-wide input sender with explicit source handles. Delete all
  associated execution and validation special cases; retain existing single-run schemas.
- Provide an explicit typed stdin JSON Lines source and a programmatic channel source so existing
  external-input use cases remain available.
- Add shared runner/CLI startup argument handling and bounded interface inspection for configuration-dependent
  ports and runtime resources. Preserve factory-free graph description and single-build installation.
- Version streaming observations for startup activation and source completion. Enable TUI launch with bounded
  invocation history, separate input routing, and stream snapshot capture disabled.

## Capabilities

### New Capabilities

- `workflow-inputs`: Derive, describe, validate, and bind startup parameters for initial workflow nodes.
- `stream-sources`: Provide explicit stdin and host-fed channel sources with typed, bounded, cancellable
  admission.

### Modified Capabilities

- `node-preparation`: Declare source resource requirements without acquiring or consuming them during
  preparation.
- `stream-node-execution`: Support startup invocations and source-local completion alongside existing
  input-driven producers.
- `workflow-stream-execution`: Schedule startup work, independent sources, message domains, and drain without
  a mandatory host input source.
- `workflow-binary-compilation`: Add the new definition schema, argument and interface protocols, and
  source-driven runner I/O.
- `workflow-observability`: Identify startup work and report completion without fabricated external inputs.
- `workflow-terminal-ui`: Launch compatible streaming runners, route required input resources, and reduce
  repeated observations with bounded memory.

## Impact

Changes affect `mf-runtime`, `mf-compiler`, `mf-telemetry`, `mf-cli`, `mf-tui`, the core node package,
generated runner APIs, and examples/documentation. Plugin metadata gains resource declarations. File parsing
and network integrations remain external plugin responsibilities. No new dependency is planned. This change
contains planning artifacts only; implementation tasks remain unchecked.
