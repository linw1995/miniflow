## Decisions

### Keep baseline and build caches separate

PR cache keys include host, source/build fingerprint, PR number, and the tested merge commit. Restore the latest compatible cache within that PR, then main. Save only the coverage database, companion SHA, commit bundle, and sparse per-test counters after successful collection. Existing build caches keep their independent paths and keys. Use GitHub's automatic retention; do not add a cleanup job or modify storage limits.

### Retain merge-commit coordinates

Keep testing the checkout's merge tree. Store a Git bundle containing the database's collection commits and the companion anchor, including their history. Import it before selection because previous PR merge refs may disappear from advertised history. This preserves actual tested source coordinates instead of replacing a merge SHA with a different head tree.

### Execute selected tests once

Use incremental collection for PRs and export sparse merged LLVM text profiles before the runner shim deletes raw profiles. Replace profiles for rerun tests, prune entries without current database mappings, and merge retained valid counters against current binaries. Reset profiles on full fallback. Missing exports for a nonempty selection fail the job; zero selected tests can reuse retained PR coverage; an empty snapshot produces no report.

The pinned 0.4.1 collector excludes explicit input rules in diff mode. Patch its selection to include those rules using the same instrumented build flags and target directory. Keep ordinary run/status behavior intact.

### Preserve ownership and feedback

PR snapshots stay under a separate namespace and PR ref scope; main never restores them. Main mapping collection and nightly complete project coverage remain unchanged. PR patch reports describe the current executed tests. An evicted, incompatible, or missing baseline retains fallback behavior.

### Verify behavior, not command strings

Extend the existing small real Rust fixture to check one execution per selected test, aggregate LCOV, preserved untouched rows, and restoration of an unadvertised merge commit into a fresh clone. Keep ablations and detailed results under ignored target directories.
