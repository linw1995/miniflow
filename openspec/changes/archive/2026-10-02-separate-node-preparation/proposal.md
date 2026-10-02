# Proposal

## Why

The current `Node` trait combines execution, metadata, and deferred body binding. Loop and Iteration
declarations must implement execution methods that always fail until another method replaces them.
Context-aware nodes also provide a second execution entry point that is never useful without a context.
Separate preparation from execution so the runtime receives complete executable nodes.

## What Changes

- Replace the execution trait with `TaskNode::execute(inputs, context) -> NodeResult`.
- Return `PreparedNode` with plain `NodeMetadata` from registration factories.
- Declare plain and subgraph factories explicitly. Subgraph factories receive the prepared body before
  constructing the runnable node.
- Migrate built-ins, compiler preparation, generated runners, and external plugin fixtures together.
- Document the breaking Rust plugin API change. Workflow JSON and existing execution semantics remain supported.

## Capabilities

### New Capabilities

- `node-preparation`: Complete prepared nodes, execution-only task contracts, and explicit factory inputs.

### Modified Capabilities

- `runtime-subgraph-execution`: Bind prepared bodies through factories instead of executable declarations.

## Impact

This is an independent base change for the streaming PR. It contains existing task and subgraph execution.
The streaming PR adds event execution on top of these preparation contracts. Plugins must migrate their
registration and execution implementations and rebuild against the matching runtime package.
