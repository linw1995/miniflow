# Verification

Controlled-clock backend parity compares the generated runner and in-memory execution for batching,
branches, chained collectors, delayed output, delivered prefixes, and synchronous bodies. Standalone
transport checks cover framing, idle-input timers, stdout isolation, stalled/broken pipes, description,
validation, cache reuse, and installed-binary protection. All eight documented examples are exercised.

- Targeted transport, parity, examples, and terminal suites: 10 tests passed.
- The full check found a minimal legacy plugin fixture still declaring the un-copied stream module.
  The fixture now strips that unused declaration; its focused integration test passed.
- All-target compilation, repository hooks, and specification validation passed.
- Corrected `nix flake check -L`: all host checks passed; 335 tests passed, none skipped.

Validation ran on `aarch64-darwin`. Stream observation is absent from this layer, while existing finite
observation remains enabled and verified. Command logs remain under ignored `target/`.
