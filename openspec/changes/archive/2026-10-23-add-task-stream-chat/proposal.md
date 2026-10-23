## Why

Chat workflows return only a complete answer today. Nodes need to publish live output while returning a final result from the same invocation, so chat can display an answer during generation and retain its existing transactional settlement.

## What Changes

- Add a combined Task/Stream execution contract with one business invocation, optional incremental emissions, and one final Task result. Both capabilities coexist; no per-node execution-mode selector is introduced.
- Separate terminal and stream output contracts and their message-domain ownership. Support explicit live exports from finite workflows and scoped bodies, and stream edges in streaming workflows.
- Accept zero emissions. Automatically discard unconnected streams without requiring a discard node; retain validation, bounded delivery, cancellation, and errors for connected streams.
- Add versioned live-output transport for embedded hosts and finite executables while preserving ordinary JSON results and existing stream runners.
- Make `builtin.llm` produce live answer deltas and its existing final outputs from one model request. Keep model composition and stream selection in the user workflow.
- Display provisional chat output, then reconcile the final result through existing save, recovery, title, and usage handlers.
- **BREAKING**: Extend prepared-provider contracts and introduce a workflow schema/manifest revision for explicit live exports. Rebuild affected providers and new streaming chat executables together; existing workflows retain their behavior.

## Capabilities

### New Capabilities

- `task-stream-node-execution`: one invocation with terminal results and optional live emissions, explicit consumption, invocation identity, and closure.

### Modified Capabilities

- `node-preparation`: reflect and construct combined executors, including finite and scoped execution.
- `stream-node-execution`: automatically discard unconnected emissions without weakening actual-emission validation.
- `workflow-runtime-execution`: schedule combined invocations and terminal publication without blocking stream progress.
- `workflow-stream-execution`: distinguish terminal and incremental output ownership and preserve drain boundaries.
- `workflow-binary-compilation`: compile live exports and expose a bounded, versioned event protocol in finite runners.
- `llm-node-execution`: publish optional text deltas and authoritative final outputs from one request.
- `chat-executable-contract`: negotiate live output without inspecting internal model nodes or changing settlement.
- `chat-workflow-execution`: expose optional live subscriptions through both backends with explicit receiver lifetime.
- `workflow-terminal-ui`: render provisional replies and reconcile completion, cancellation, failure, and save recovery.

## Impact

Changes span `mf-runtime`, `mf-compiler`, `mf-telemetry` descriptions, `mfn-llm`, `mf-chat`, `mf-tui`, generated-runner fixtures, examples, and documentation. Storage schemas and transactional completion remain unchanged. The implementation first validates the pinned Rig streaming behavior; a dependency upgrade is not assumed.
