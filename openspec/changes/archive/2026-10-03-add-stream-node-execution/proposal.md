# Proposal

## Why

A file-reading node needs to emit each line while downstream work is running. Event callbacks currently return a complete vector and reject emissions above the pending-message limit, so they cannot provide bounded incremental output for a large file.

## What Changes

- Add a prepared stream executor with an incremental, blocking output emitter.
- Run each active stream executor on a reusable dedicated producer thread, independently of ordinary task workers and the coordinator.
- Apply the existing per-operator message limit as backpressure on each send, including failure wakeups and close-and-drain behavior.
- Preserve typed publication, message-domain isolation, context access, observation, and compiled runner parity.
- **BREAKING**: Extend the public execution and stream error enums; exhaustive downstream matches need updating.

## Capabilities

### New Capabilities

- `stream-node-execution`: Incremental one-to-many execution with bounded blocking emission and instance-owned producers.

### Modified Capabilities

- `node-preparation`: Prepare stream executors and reject them from synchronous scopes.
- `workflow-stream-execution`: Include producer invocations in message domains, draining, and failure cleanup.

## Impact

Changes affect `mf-runtime`, compiler preparation, integration fixtures, and node-development documentation. The existing worker pool is reused for dedicated producer workers. No new dependency or workflow schema version is required. File access and line parsing remain the responsibility of `mfn-rag`, outside this change.
