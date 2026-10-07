## ADDED Requirements

### Requirement: Preserve ownership metadata parsing errors

The compiler SHALL reject invalid build ownership metadata with a source-bearing parsing error that identifies the `.mf-owner.json` path and retains the original `serde_json::Error` in its error chain.

#### Scenario: Reject a corrupted ownership marker

- **WHEN** a selected build directory contains a `.mf-owner.json` marker that cannot be deserialized
- **THEN** opening the build directory fails before cached artifacts are reused
- **AND** the error identifies the marker path and exposes the original JSON parsing error through `Error::source()`
