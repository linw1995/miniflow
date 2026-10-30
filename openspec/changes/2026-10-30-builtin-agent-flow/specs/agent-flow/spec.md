# Agent Flow requirements

## Explicit LLM responsibility

Each LLM execution SHALL perform one model request using its ordinary tools input. It SHALL NOT
route workflow arguments or require a particular downstream container.

## Ordinary dispatch

Dispatch SHALL select a declared branch from its selector, publish that branch gate and parameter
ports, and mark the other branches skipped. It SHALL NOT execute child nodes. Required routed values
and unknown arguments SHALL be checked before publication. Ordinary workflow nodes SHALL execute
selected tools and downstream processing.

## Prepared tool definitions

The agent SHALL prepare model tool definitions before requesting the model. Schemas and sensitivity
SHALL come from dispatch-linked receiver ports; unrelated upstream inputs SHALL remain excluded.
Incomplete codec schemas SHALL fail before model I/O. Reusable tool workflow interfaces SHALL retain
their qualified input names.

## Shared orchestration

The provider-owned built-in agent SHALL use one Iteration for the complete call list and an ordinary
Loop for model rounds. Repeated and mixed tool names SHALL preserve model order in protocol results.
Per-call routing fields SHALL override shared tool inputs. Limits SHALL bound model rounds and calls
per round. Existing workflow step budgets and error policies SHALL remain authoritative.

## Portable Flow composition

A caller SHALL be able to bind the built-in agent's tool Flow source. Binding paths SHALL resolve
relative to the caller. Every body SHALL be frozen before runtime, and execution SHALL remain
possible after source Flow files are removed.

## Observation and streams

Ordinary tool nodes, Calls, Loops, and Iteration items SHALL remain visible to existing observation.
Optional text deltas SHALL cross existing child-stream exports. An unobserved text stream SHALL NOT
force an otherwise unary workflow request. Sensitive argument and result values SHALL retain their
markers and be redacted at observation and protocol-result boundaries.
