# Review

The final implementation follows the repository's module and error conventions.

- CLI flags remain in `mf-cli`; orchestration and rendering remain in the existing `mf-tui` modules. Public APIs
  stay at their module boundaries, without visibility facades or relocation of tests for access control.
- The terminal reuses mainline executable contracts, input resolution, database storage, result reconciliation,
  message metadata, and per-call usage. It adds no runtime protocol, model topology inspection, or storage schema.
- Ordinary observation and chat share process supervision and observation rendering. Persistent transcript facts
  remain independent of child exit codes, output diagnostics, and telemetry completeness.
- Errors preserve typed sources through transparent variants and Snafu selectors. Text summaries are used only for
  durable failure records and user-facing diagnostics. Close returns cleanup failures as an error.
- Session reservations cover active and retained replies. Save retry never launches a workflow; shutdown joins
  children before releasing ownership. Failure to access storage is reported and does not silently claim success.
- Retained observation readers close on exceptional teardown. Retention follows active invocation ownership,
  including runs that ended without a process status, and does not evict active work.
- Dependencies remain workspace-managed. Experimental scripts and reports stay under the ignored `target/` tree.

No unresolved implementation or specification findings remain. Final validation is recorded separately.
