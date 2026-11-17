# Configure Flow inputs from producer context

## Why

Tool Flows need the same context-template configuration available to root-Flow nodes. Configured inputs, including public settings, are application-owned values rather than model arguments. An Agent-specific private argument map duplicates ordinary Flow binding and cannot express that residual interface cleanly.

## What changes

- Add `config.inputs` to native Flow constants, using the existing `$ref` and `$template` mechanism.
- Bind explicit input values immutably when the producer executes, without retaining its context.
- Project only unconfigured public inputs into the model schema and reject all caller overrides of configured inputs.
- Share value merging, typing, and secret propagation across native Flow calls and frozen Router calls.
- Remove the provisional Agent `tool_arguments` channel and `llm.private_arguments` node.
- Configure each tool capability before publication without requiring an application-specific Agent.

## Impact

Unconfigured Flow constants and existing Agent callers retain their interfaces. Parameter defaults and environment declarations remain ordinary overridable fallback sources. Prepared definitions remain frozen, and configured native values do not become JSON executable data or model arguments. The provisional private-map API in this unmerged change is replaced rather than retained as a second configuration path.

## Non-goals

- Capturing an entire producer context or implicitly reading ambient caller outputs.
- Model-controlled credentials or overrides of fixed public settings.
- Runtime replacement of compiled bodies, environment mutation, or provider retries.
