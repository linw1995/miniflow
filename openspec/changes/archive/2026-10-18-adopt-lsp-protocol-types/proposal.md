## Why

The LSP client manually constructs and interprets standard protocol payloads. Shared protocol types can remove duplicated schema knowledge and reject malformed evidence before planning consumes it.

## What Changes

- Adopt `lsp-types` for standard semantic requests, results, notifications, and capabilities.
- Retain bounded transport, process ownership, cancellation, partial-result aggregation, and analyzer-specific readiness.
- Keep the existing Flow ports and serialized evidence format.

## Capabilities

### Modified Capabilities

- `lsp-analysis`: validate standard semantic messages using shared protocol types.

## Impact

Only unpublished developer tooling gains the dependency. Existing protocol fixtures and the real rust-analyzer smoke check cover the migration.

## Non-goals

No new Flow nodes, async runtime, transport backend abstraction, or test-selection policy changes.
