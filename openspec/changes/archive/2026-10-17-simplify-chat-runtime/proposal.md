## Why

Configuration fingerprints do not participate in execution, ownership, or recovery. Keeping them mandatory adds hashing, API fields, and statistics dimensions without an established consumer. Recovery also unnecessarily requires a manifest-derived contract although result decoding uses none of its fields.

## What Changes

- Remove fingerprint generation, public identity fields, query filters/grouping, and the chat package hash dependency.
- Preserve historical fingerprint values as inert database data; new records omit them.
- Decode and reconcile results without an executable contract.
- Remove duplicate profile-name storage, repeated decode validation, and fingerprint-specific test scaffolding.

## Impact

This changes the mf-chat Rust API and usage query schema. Existing persisted calls, usages, metadata, and lifecycle behavior remain supported.
