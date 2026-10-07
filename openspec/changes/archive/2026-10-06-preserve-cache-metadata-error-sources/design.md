## Decisions

- Attach `MetadataParseSnafu` directly to ownership marker deserialization. Keep its path and typed JSON source separate from encoding failures and semantic ownership mismatches.
- Add no parsing helper or error abstraction.
- Keep one corrupted JSON regression checking the typed source and marker path. Existing cache tests cover incompatible layouts; JSON classification and positions are provided by `serde_json`.

## Review

Restore the original string-only mapping as a negative control, then remove the regression test to check whether existing coverage detects source loss. Keep experiment scripts and reports under ignored `target/`. Review the retained error boundary and validate the implementation and specification before archival.
