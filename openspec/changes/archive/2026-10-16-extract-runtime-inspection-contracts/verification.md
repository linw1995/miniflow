# Verification

- The pinned workspace check passed for all targets and features.
- The affected-package selector chose nine packages; all 683 selected tests passed, with four default skips.
- The selected suites cover relocated Task, Stream, and nested Loop runners, release/LTO/stripping, telemetry-disabled
  builds, embedded hosts, TUI execution, and provider input-schema drift.
- Current manifest tests cover required source facts, exact node IDs, compiled Loop order, input contracts, typed JSON
  errors, payload bounds, and event-protocol derivation without a serialized duplicate field.
- Both negative controls failed as expected; all seven focused manifest tests passed after restoring the source.
- Archived deltas pass strict OpenSpec validation. The changed main specification passes ordinary validation;
  its twelve inherited requirement-length warnings remain unchanged.

Experimental sources and reports remain local under ignored `target/pre-release-ablation/`. The archive and main
specification require compiled facts and derived inspection views without providers, replanning, or historical manifest
compatibility. Existing Flow language versions retain their execution semantics.
