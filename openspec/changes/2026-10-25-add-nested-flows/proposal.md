## Why

Reusable workflows currently require copying their nodes into each caller. Chat title generation is a
concrete example: eligibility, prompting, error handling, and usage accounting obscure the primary flow.

## What Changes

- Add compile-time local and built-in Flow references and a terminal `workflow.call` container.
- Derive call ports from the child startup schema and named outputs, preserving typed shared values.
- Isolate child inputs and outputs while sharing cancellation, workers, futures, and step budgets.
- Describe and observe calls as ordinary nested scopes across dynamic and generated execution.
- Supply the built-in chat title Flow and simplify the composed chat example.

## Impact

Extend workflow authoring and observation versions, compiler resolution and generation, runtime scope
mechanics, core node providers, embedding, and the chat examples. Streaming ports across a call boundary
and recursive Flow invocation are outside this change.
