## Why

Chat usage records preserve token counts but cannot explain latency or generation throughput.
Streaming calls need a first-text observation while unary and historical calls must retain unknown FTT.

## What Changes

- Record monotonic request duration and optional first-text latency in each LLM usage fact.
- Derive end-to-end and post-first-text output-token rates from authoritative reported counters.
- Persist timing with the existing response transaction and expose weighted performance aggregates.
- Preserve sparse observations, historical records, failed attempts, and save-only retry semantics.

## Impact

The change extends LLM usage contracts, the chat usage view and query, and their documentation.
It does not estimate tokens from streamed chunks or introduce additional provider calls.
