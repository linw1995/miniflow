# Recover checked web-tool failures

## Why

A real Agent run successfully searched the web, then a Fetch HTTP 404 aborted the entire workflow without a terminal conversation result. The UI repeated wrapper diagnostics instead of showing a completed tool exchange. A model should be able to react to a checked tool-operation failure without swallowing cancellation or unknown runtime failures.

## What changes

- Add a source-bearing runtime business-failure category for providers to classify checked domain failures explicitly.
- Let packed Call/Router boundaries encode that category as an existing tool-result error.
- Classify DeepSeek public-input validation failures and non-success HTTP statuses, while preserving operational failures as errors.
- Deduplicate exact terminal diagnostic lines in the chat presentation only.
- Verify a compiled Agent observes a Fetch 404, chooses a different source explicitly, and returns a complete terminal transcript.

## Non-goals

- Catching arbitrary plugin failures or losing typed error chains.
- HTTP retries, redirect following, or changing provider success outputs.
- Rewriting or deduplicating persisted audit data and raw logs.
