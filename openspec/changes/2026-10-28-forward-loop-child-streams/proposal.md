## Why

A finite Loop body can execute TaskStream nodes but its terminal adapter discards child emissions.
Workflow-owned iteration therefore cannot publish incremental answers through the normal chat contract.

## What Changes

- Add explicit, versioned Loop stream exports using existing output references.
- Forward child emissions synchronously through existing TaskStream emitters and sinks.
- Share prepared success guards and ordinary execution lifecycle across terminal and forwarded execution.
- Preserve native Loop summaries and freeze child stream contracts in generated runners.
- Enable the existing chat Loop example to publish answer previews.

## Impact

This change is based on origin/main and does not depend on directed calls or LLM ToolUse changes.
Loop variables and terminal behavior remain unchanged when no stream output is consumed. The runtime
adds a default task sink entry point and a sink-aware prepared subgraph. Public Loop definitions gain
an optional stream export map; task-stream completion exposes the optional Loop summary.
