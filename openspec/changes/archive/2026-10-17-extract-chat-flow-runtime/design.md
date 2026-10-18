## Context

This change extracts the backend from `feat/executable-chat-tui` at `6badf1c` onto the current mainline
manifest and input-resolution APIs. The caller supplies an inspected manifest and owns runner processes.

## Decisions

- Keep the existing `mf-chat` module boundary and public executable API. Do not introduce another crate,
  facade, process controller, or runtime-specific chat error.
- Validate the manifest and chat-specific topology before admitting a conversation. Derive configuration
  inputs by removing the directly connected conversation inputs from the prepared schema.
- Resolve model identity from the executable configuration and public resolved parameters. Exclude
  resolved secrets from the accounting fingerprint and reject secret model names.
- Admit caller-allocated invocation IDs through the existing turn node. Use the existing transactional
  completion and failure nodes for persistence; preserve committed success during reconciliation.
- Keep durable invocation state distinct from a retained, uncommitted reply. Report output diagnostics
  independently from persistence state. Save-only retries retain the original reply on write failure.
- Use the same conversation save-retry implementation for executable callers and the embedded host.
  The host continues to enforce shutdown state and dispatch automatic titles after successful saves.

## Limits

Reply decoding is bounded at 16 MiB. The application must also bound capture before decoding. Recovery
is memory-only: a crash can lose an unsaved reply, and reopening interrupts abandoned work without resend.
The caller owns process cancellation, concurrency admission, and pending-title scheduling.
