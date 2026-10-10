# Proposal

## Why

Workflows need a builtin task that turns a prompt and explicit conversation history into a completed model response and an updated transcript for the next turn. Rig provides the provider protocol integration, while miniflow needs an optional future execution boundary that preserves its synchronous task contracts and keeps executor dependencies out of ordinary workflows.

## What Changes

- Add the explicitly selected `mfn-llm` package registering `builtin.llm`, using OpenAI-compatible Chat Completions and publishing one terminal result per successful invocation.
- Reflect fixed `prompt`, optional `system`, and optional text-message `history` inputs, with complete `text`, updated `history`, and optional `usage` and `finish_reason` outputs through the existing typed task contract.
- Let workflows carry history between turns. Preserve message order, append the current user prompt and assistant reply once on success, and keep system instructions separate from the returned history.
- Configure model, base URL, credential environment-variable name, generation parameters, request deadline, and output text limit. Read credentials and initialize transport only during execution.
- Add optional future execution support to `mf-runtime`, with a portable blocking executor and an opt-in managed Tokio backend. Public task and context APIs do not expose Tokio types.
- Preserve existing preparation, generated execution, skip behavior, and publication checks. Do not add streaming outputs, tool execution, implicit node-owned conversation memory, structured generation, or node-specific compiler dispatch.

## Capabilities

### New Capabilities

- `llm-node-execution`: Configure and execute terminal text generation with explicit conversation history through an explicitly selected builtin node.
- `runtime-future-execution`: Drive node-owned futures through an optional execution facility without replacing synchronous task scheduling.

### Modified Capabilities

None. Existing typed preparation, dependency feature selection, and task publication contracts already support the node.

## Impact

- New `crates/builtin-nodes/llm/` package and workspace entries for Rig and its transport dependencies.
- Optional executor dependencies and execution-context resource ownership in `mf-runtime`; no Rig dependency in the runtime, compiler, CLI, or `mfn-core`.
- Workflow and node development documentation, terminal-generation examples, release support, packaged CLI acceptance, and license notices.
- No workflow schema change. Selecting the LLM package adds its native HTTP transport's Tokio dependency, even when the portable future executor is used.
