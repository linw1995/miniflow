## Why

Invalid `.mf-owner.json` metadata is reported as an ownership reason string, losing the original JSON error from the source chain.

## What Changes

- **BREAKING**: preserve JSON decoding failures in a new source-bearing `CacheError::MetadataParse` variant with the ownership marker path.
- Retain one focused regression after implementation and test ablations.

## Capabilities

### Modified Capabilities

- `workflow-binary-compilation`: preserve typed ownership metadata parsing errors.

## Impact

Callers can inspect the original `serde_json::Error`. Adding a public error variant affects exhaustive matches on `CacheError`.
