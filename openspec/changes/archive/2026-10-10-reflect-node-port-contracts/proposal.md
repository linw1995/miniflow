## Why

Factories can maintain a port list separately from the executor's input and output contracts. Fixed output overrides
also mix interface declaration with type inference, allowing duplicate sources of truth.

## What Changes

- Reflect fixed input/output value contracts and validated dynamic executor contracts during preparation.
- Use one `NodePortContract` for task, event, and stream providers, while typed tasks retain automatic adaptation.
- Keep literal, forwarding, collection, and proven-type evidence separate from reflected port declarations.
- Migrate builtin providers and keep one explicit low-level assembly entry point for existing execution and metadata.
- **BREAKING**: `PreparedNode::new`, `event`, and `stream` require `NodePortContract` and return `Result`;
  `TypedTaskNode::output_ports` is removed. Existing raw assembly uses `from_parts(NodeExecution, metadata)`.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `node-preparation`: reflect interfaces for all executor kinds and separate output evidence from declarations.
- `typed-port-contracts`: validate proven-type evidence and reflect fixed builtin bags without factory overrides.

## Impact

Changes affect `mf-runtime`, the compiler's existing type inference, builtin providers, validation fixtures, and
node development documentation. Workflow JSON and execution protocols remain unchanged. No dependencies are added.
