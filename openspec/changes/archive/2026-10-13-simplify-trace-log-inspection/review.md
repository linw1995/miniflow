## Findings and resolutions

- Empty Details previously cached the collecting message after completion. One view now transitions through collecting and finished states in the existing regression test, and mutable absence text refreshes directly.
- A global snapshot revision invalidated Data for unrelated packet traffic. Data now derives from the current snapshot; immutable span/log previews retain their cache.
- Snapshot export manually repeated the pinned SDK logger's active context injection. That duplicate setter is removed while native context and fragmented-export regression coverage remains.
- Resource fingerprint coverage previously changed scope alongside resource. The existing test now varies resource identity independently and rejects the corresponding mutation.
- Overlapping UTF-8 preview checks are consolidated. Cache-pointer assertions are removed; ordering, immutable historical contents, selection, retention bounds, and complete-session access remain covered.

## Repository rule review

The change preserves module entry points and shared public APIs without facade modules or mechanical scoped visibility. The removed revision field was introduced by this feature and has no role in the observation contract. Snapshot errors remain Snafu-derived with typed sources, cached source chains, and selector-based invariant checks. Node-specific runtime errors and execution contracts are unchanged.

The pinned development shell and installed hooks are used for checks. Generated measurements, mutation logs, profiles, and test mappings stay under ignored `target/`. User-owned untracked files remain untouched. Commit naming follows the allowed type/scope format; no amend, force push, or remote action is required.

## Review outcome

No unresolved implementation or specification issue was found after the corrections. Existing trace/log/data requirements remain accurate. Archive follows final validation; this follow-up declares no spec delta.
