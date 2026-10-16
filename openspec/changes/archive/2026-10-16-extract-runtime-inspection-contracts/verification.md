# Verification

- Pinned workspace checks passed for all targets and features.
- 145 focused runtime/compiler/TUI tests passed. Manifest construction and helper contracts passed focused checks.
- Both standalone binary tests passed. The embedded-host test passed after its remaining field accesses were migrated.
- The affected-package selector chose nine packages. Reusing those three completed tests, all other 683 tests passed:
  686 distinct tests passed in total, with four default skips. The existing CI profile reused generated-runner caches.
- Fixed workflow identity, interface versions, and event schemas are tested for the current manifest format.
- The repository coverage script passed 27 focused tests. All 186 modified executable runtime-manifest lines were covered.
- With dependency versions matching Cargo.lock, compact manifest JSON decreased by 45.4%, 38.5%, and 38.9% for the hello,
  Loop, and stream examples. These are payload measurements, not executable-size measurements.
- Removing strict shape validation or discarding frozen node order failed behavioral controls; restored code passed.
- Final pinned `prek -a` passed every hook on frozen content, including formatting, Clippy, Cargo check, rust-analyzer,
  Markdown, workflow validation, and the license audit.
- Archived deltas pass strict OpenSpec validation. The changed main specification passes ordinary validation;
  its twelve inherited requirement-length warnings remain unchanged in strict mode.

All experimental sources and reports remain local under ignored `target/manifest-views/`. The final review confirms
one contract in the archive and main specification: retain compiled facts, derive versioned inspection views without
providers or replanning, and preserve legacy reads through the same helper API.
